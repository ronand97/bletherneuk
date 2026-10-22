"""bletherneuk iTerm2 bridge.

Runs from iTerm2's AutoLaunch folder on iTerm2's bundled Python runtime.
It translates requests from the bletherneuk service into iTerm2 API calls
and reports iTerm2 events back. It holds no logic of its own.

The contract is docs/protocol.md section 1.2. In outline:

- `run_link` keeps one connection to the service's Unix socket. It sends
  `bridge_hello`, then answers each `request` line in order with a `reply`.
  It reconnects with backoff (up to every 30 seconds) when the service is down.
- iTerm2 monitors (focus, new session, layout, session end, theme) feed
  `focus`, `session_opened`, `session_closed`, `window_closed` and `colors`.
- Everything the service sends is validated before any iTerm2 call
  (design section 20.6). Folders reach iTerm2 only as profile settings.
- The `bletherneuk_open_board()` RPC sends a one-shot `open_page` request
  on a new socket connection (protocol section 1.1).

Never log URLs (they carry one-time codes), prompts or hook data.
"""

from __future__ import annotations

import asyncio
import json
import os
import plistlib
import re
import stat
import sys
import unicodedata

import iterm2

BRIDGE_VERSION = "0.1.0"
MAX_LINE = 1024 * 1024
REQUEST_TIMEOUT = 60
MIN_BROWSER_VERSION = (3, 7, 6)
# iterm2 2.30 (capabilities.py) ties API protocol 1.23 to iTerm2 3.7.6.
MIN_BROWSER_PROTOCOL = (1, 23)
BLOCK_VAR = "user.bletherneuk_block"
CHAT_VAR = "user.bletherneuk_chat"
PANEL_NAME = "bletherneuk"
PANEL_ID = "dev.bletherneuk.panel"
MAX_TITLE = 200
MAX_ITEMS = 32

UUID_RE = re.compile(r"[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}")
# Stricter than protocol.md: the first character must be a letter or digit, so a
# name can never be read as an option (`--worktree --some-flag`) or be `.`/`..`.
WORKTREE_RE = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]{0,63}")
URL_RE = re.compile(r"http://127\.0\.0\.1:([0-9]{1,5})/[\x21-\x7e]*")
BIDI = frozenset("\u200e\u200f\u061c\u202a\u202b\u202c\u202d\u202e\u2066\u2067\u2068\u2069")


def log(text):
    print("bletherneuk: " + text, file=sys.stderr, flush=True)


class Refused(Exception):
    """A request the bridge won't carry out. The argument is the reply's error."""


def need(ok, error):
    if not ok:
        raise Refused(error)


# Validation (design section 20.6) ------------------------------------------


def is_uuid(value):
    return isinstance(value, str) and UUID_RE.fullmatch(value) is not None


def is_block(value):
    return isinstance(value, int) and not isinstance(value, bool) and 0 <= value < 2**53


def has_unsafe_chars(text):
    return any(unicodedata.category(c) == "Cc" or c in BIDI for c in text)


def is_safe_folder(folder):
    if not isinstance(folder, str) or not folder.startswith("/") or len(folder) > 1024:
        return False
    if has_unsafe_chars(folder):
        return False
    try:
        info = os.stat(folder)
    except (OSError, ValueError):
        return False
    return stat.S_ISDIR(info.st_mode) and info.st_uid == os.getuid()


def is_worktree_name(name):
    return isinstance(name, str) and WORKTREE_RE.fullmatch(name) is not None


def is_page_url(url):
    if not isinstance(url, str) or len(url) > 4096:
        return False
    match = URL_RE.fullmatch(url)
    return match is not None and 1 <= int(match.group(1)) <= 65535


def clean_title(title):
    """Makes a title safe to set: iTerm2 evaluates `\\(...)` in titles."""
    need(isinstance(title, str), "invalid_title")
    kept = "".join(c for c in title if not (unicodedata.category(c) == "Cc" or c in BIDI))
    return kept[:MAX_TITLE].replace("\\", "\\\\")


def optional_title(title):
    return None if title is None else clean_title(title)


# Paths, versions and colours ------------------------------------------------


def socket_path(env=None):
    """The service socket, chosen exactly as src/paths.rs chooses it."""
    env = os.environ if env is None else env
    home = env.get("BLETHERNEUK_HOME") or env.get("HOME") or os.path.expanduser("~")
    default = os.path.join(home, "Library", "Application Support", "bletherneuk", "run", "blether.sock")
    if len(os.fsencode(default)) <= 103:
        return default
    return os.path.join(env.get("TMPDIR") or "/tmp", "bletherneuk", "blether.sock")


def version_tuple(text):
    match = re.match(r"([0-9]+)\.([0-9]+)(?:\.([0-9]+))?", text) if isinstance(text, str) else None
    return tuple(int(part or 0) for part in match.groups()) if match else None


def browser_supported(iterm_version, protocol_version):
    parsed = version_tuple(iterm_version)
    if parsed is not None:
        return parsed >= MIN_BROWSER_VERSION
    return tuple(protocol_version) >= MIN_BROWSER_PROTOCOL


def hex_color(color):
    def channel(value):
        return max(0, min(255, int(round(value))))

    return "#%02x%02x%02x" % (channel(color.red), channel(color.green), channel(color.blue))


def profile_colors(profile, dark):
    suffix = ""
    if profile.use_separate_colors_for_light_and_dark_mode:
        suffix = " (Dark)" if dark else " (Light)"
    keys = ["Background Color", "Foreground Color"] + ["Ansi %d Color" % i for i in range(16)]
    colors = [profile.get_color_with_key(key + suffix) for key in keys]
    if any(color is None for color in colors):
        return None
    hexes = [hex_color(color) for color in colors]
    return {"background": hexes[0], "foreground": hexes[1], "ansi": hexes[2:]}


# iTerm2 helpers --------------------------------------------------------------


def terminal_profile(folder):
    profile = iterm2.LocalWriteOnlyProfile()
    profile._simple_set("Custom Directory", "Yes")
    profile._simple_set("Working Directory", folder)
    return profile


def browser_profile(url):
    profile = iterm2.LocalWriteOnlyProfile()
    profile._simple_set("Custom Command", "Browser")
    profile._simple_set("Initial URL", url)
    return profile


def tab_sessions(tab):
    # all_sessions (newer packages) adds minimized and floating panes.
    return getattr(tab, "all_sessions", None) or tab.sessions


def current_session(app):
    window = app.current_window
    tab = window.current_tab if window is not None else None
    return tab.current_session if tab is not None else None


def current_focus(app):
    window, session = app.current_window, current_session(app)
    return {
        "session": session.session_id if session is not None else None,
        "window": window.window_id if window is not None else None,
    }


def block_from_variable(value):
    if is_block(value):
        return value
    if isinstance(value, float) and value.is_integer() and is_block(int(value)):
        return int(value)
    if isinstance(value, str) and re.fullmatch(r"[0-9]{1,15}", value):
        return int(value)
    return None


def encode(message):
    return (json.dumps(message, separators=(",", ":")) + "\n").encode()


class Bridge:
    def __init__(self, connection, path=None):
        self.connection = connection
        self.path = path or socket_path()
        self.windows = {}  # window ID -> block ID or None
        self.sessions = {}  # session ID -> {"window", "tab", "chat"}
        self.splits = {}  # tab ID -> browser session showing the Changes page
        self.board = None  # browser session showing the board
        self.probes = set()  # browser sessions opened for `blether doctor --live`
        self.focus = None
        self.colors = None
        self.iterm_version = "unknown"
        self.browser_ok = False
        self.writer = None
        self.tasks = set()
        self.lock = asyncio.Lock()  # guards the caches and the order of events
        self.send_lock = asyncio.Lock()
        self.ops = {
            "panel": self.op_panel,
            "open_chat": self.op_open_chat,
            "new_chat": self.op_new_chat,
            "open_block": self.op_open_block,
            "open_page": self.op_open_page,
            "set_titles": self.op_set_titles,
            "close_session": self.op_close_session,
            "ping": self.op_ping,
        }

    async def app(self):
        return await iterm2.async_get_app(self.connection)

    def spawn(self, coro):
        task = asyncio.ensure_future(coro)
        self.tasks.add(task)
        task.add_done_callback(self.tasks.discard)

    async def start(self):
        app = await self.app()
        try:
            bundle = await app.async_get_variable("appBundlePath")
            need(isinstance(bundle, str) and bundle.startswith("/"), "no bundle path")
            with open(os.path.join(bundle, "Contents", "Info.plist"), "rb") as handle:
                self.iterm_version = str(plistlib.load(handle)["CFBundleShortVersionString"])
        except Exception:
            self.iterm_version = "unknown"
        self.browser_ok = browser_supported(self.iterm_version, self.connection.iterm2_protocol_version)
        await self.register_rpc()
        c = self.connection
        theme = iterm2.VariableMonitor(c, iterm2.VariableScopes.APP, "effectiveTheme", None)
        self.spawn(self.watch(iterm2.FocusMonitor(c), "async_get_next_update", self.on_focus))
        self.spawn(self.watch(iterm2.NewSessionMonitor(c), "async_get", self.on_layout))
        self.spawn(self.watch(iterm2.LayoutChangeMonitor(c), "async_get", self.on_layout))
        self.spawn(self.watch(iterm2.SessionTerminationMonitor(c), "async_get", self.on_terminated))
        self.spawn(self.watch(theme, "async_get", self.on_theme))
        self.spawn(self.run_link())

    async def register_rpc(self):
        bridge = self

        @iterm2.RPC
        async def bletherneuk_open_board():
            message = {"type": "open_page", "page": "board", "block": None, "near_session": None}
            bridge.spawn(bridge.ask_service(message))

        await bletherneuk_open_board.async_register(self.connection)

    # Service connection -----------------------------------------------------

    async def open_socket(self):
        info = os.lstat(self.path)
        if not stat.S_ISSOCK(info.st_mode) or info.st_uid != os.getuid():
            raise OSError("not the service's socket")
        connect = asyncio.open_unix_connection(self.path, limit=MAX_LINE + 2)
        return await asyncio.wait_for(connect, 5)

    async def run_link(self):
        loop = asyncio.get_running_loop()
        delay = 1
        while True:
            started = loop.time()
            try:
                reader, writer = await self.open_socket()
            except (OSError, asyncio.TimeoutError):
                await asyncio.sleep(delay)
                delay = min(delay * 2, 30)
                continue
            log("connected to the service")
            try:
                app = await self.app()
                async with self.lock:
                    writer.write(encode(await self.hello(app)))
                    await writer.drain()
                    self.writer = writer
                await self.read_requests(reader)
            except Exception as exc:
                log("service connection ended: " + type(exc).__name__)
            finally:
                self.writer = None
                writer.close()
            log("disconnected from the service")
            if loop.time() - started > 30:
                delay = 1
            await asyncio.sleep(delay)
            delay = min(delay * 2, 30)

    async def read_requests(self, reader):
        while True:
            line = await reader.readline()
            if not line:
                return
            try:
                message = json.loads(line)
            except ValueError:
                continue
            if isinstance(message, dict) and message.get("type") == "request":
                await self.handle(message)

    async def send(self, message):
        writer = self.writer
        if writer is None:
            return
        async with self.send_lock:
            try:
                writer.write(encode(message))
                await writer.drain()
            except (OSError, RuntimeError):
                pass

    async def ask_service(self, message):
        """Sends one one-shot request (protocol section 1.1) and reads its reply."""
        try:
            reader, writer = await self.open_socket()
            try:
                writer.write(encode(message))
                await writer.drain()
                reply = json.loads(await asyncio.wait_for(reader.readline(), 30) or b"{}")
            finally:
                writer.close()
            if reply.get("ok") is not True:
                log("the service could not open the page")
        except Exception as exc:
            log("could not reach the service: " + type(exc).__name__)

    # Reports ----------------------------------------------------------------

    async def hello(self, app):
        """Builds bridge_hello and resets the caches. Call with self.lock held."""
        self.windows, self.sessions = {}, {}
        await self.sync(app, announce=False)
        sessions = [
            {"session": sid, "window": info["window"], "tab": info["tab"],
             "block": self.windows.get(info["window"]), "chat": info["chat"]}
            for sid, info in self.sessions.items()
        ]
        self.focus = current_focus(app)
        self.colors = await self.read_colors(app)
        return {"type": "bridge_hello", "bridge_version": BRIDGE_VERSION,
                "iterm_version": self.iterm_version, "colors": self.colors,
                "sessions": sessions, "focus": self.focus}

    async def sync(self, app, announce=True):
        """Brings the caches up to date with iTerm2. Call with self.lock held."""
        live = set()
        for window in app.windows:
            live.add(window.window_id)
            if window.window_id not in self.windows:
                self.windows[window.window_id] = block_from_variable(await window.async_get_variable(BLOCK_VAR))
            for tab in window.tabs:
                for session in tab_sessions(tab):
                    where = {"window": window.window_id, "tab": tab.tab_id}
                    info = self.sessions.get(session.session_id)
                    if info is not None:
                        info.update(where)
                        continue
                    chat = await session.async_get_variable(CHAT_VAR)
                    self.sessions[session.session_id] = dict(where, chat=chat if is_uuid(chat) else None)
                    if announce:
                        await self.send(dict(where, type="session_opened", session=session.session_id, block=None))
        for window_id in [w for w in self.windows if w not in live]:
            block = self.windows.pop(window_id)
            gone = [sid for sid, info in self.sessions.items() if info["window"] == window_id]
            chats = [self.sessions.pop(sid)["chat"] for sid in gone]
            if announce:
                await self.send({"type": "window_closed", "window": window_id, "block": block,
                                 "chats": [chat for chat in chats if chat]})

    async def read_colors(self, app):
        try:
            session = current_session(app)
            if session is not None:
                profile = await session.async_get_profile()
            else:
                profile = await iterm2.Profile.async_get_default(self.connection)
            theme = await app.async_get_variable("effectiveTheme")
            return profile_colors(profile, "dark" in str(theme).split())
        except Exception:
            return None

    async def send_colors(self, app):
        colors = await self.read_colors(app)
        if colors is not None and colors != self.colors:
            self.colors = colors
            await self.send({"type": "colors", "colors": colors})

    async def watch(self, monitor, getter, handler):
        try:
            async with monitor as mon:
                while True:
                    value = await getattr(mon, getter)()
                    try:
                        await handler(value)
                    except Exception as exc:
                        log("event handling failed: " + type(exc).__name__)
        except Exception as exc:
            log("could not watch iTerm2: " + type(exc).__name__)

    async def on_focus(self, _update):
        app = await self.app()
        async with self.lock:
            await self.sync(app)  # a closed window often shows up first as a focus change
            focus = current_focus(app)
            if focus != self.focus:
                self.focus = focus
                await self.send(dict(focus, type="focus"))
                await self.send_colors(app)

    async def on_layout(self, _value):
        app = await self.app()
        async with self.lock:
            await self.sync(app)

    async def on_terminated(self, session_id):
        app = await self.app()
        async with self.lock:
            await self.sync(app)  # report a closed window, with its chats, first
            self.sessions.pop(session_id, None)
            await self.send({"type": "session_closed", "session": session_id})

    async def on_theme(self, _value):
        app = await self.app()
        async with self.lock:
            await self.send_colors(app)

    # Requests ---------------------------------------------------------------

    async def handle(self, message):
        request_id = message.get("id")
        if not isinstance(request_id, int) or isinstance(request_id, bool):
            return
        session, error = None, None
        op = self.ops.get(message.get("op"))
        try:
            need(op is not None, "unknown_op")
            session = await asyncio.wait_for(op(message), REQUEST_TIMEOUT)
        except Refused as exc:
            error = str(exc)
        except asyncio.TimeoutError:
            error = "timeout"
        except Exception as exc:
            log("request failed: " + type(exc).__name__)
            error = "iterm2_error"
        await self.send({"type": "reply", "id": request_id, "ok": error is None,
                         "error": error, "session": session})

    def block_window(self, app, block):
        for window in app.windows:
            if self.windows.get(window.window_id) == block:
                return window
        return None

    def session_for_chat(self, app, chat):
        for sid, info in self.sessions.items():
            if info["chat"] == chat:
                return app.get_session_by_id(sid)
        return None

    async def show(self, app, session):
        await session.async_activate(select_tab=True, order_window_front=True)
        await app.async_activate(raise_all_windows=False)

    async def new_tab(self, app, block, profile, window_title=None, tab_title=None):
        """Opens a tab in the block's window, creating the window if needed."""
        window = self.block_window(app, block)
        if window is None:
            window = await iterm2.Window.async_create(self.connection, profile_customizations=profile)
            need(window is not None, "iterm2_error")
            await window.async_set_variable(BLOCK_VAR, block)
            self.windows[window.window_id] = block
            tab = window.tabs[0]
        else:
            tab = await window.async_create_tab(profile_customizations=profile)
            need(tab is not None, "iterm2_error")
        session = tab.sessions[0]
        where = {"window": window.window_id, "tab": tab.tab_id}
        self.sessions[session.session_id] = dict(where, chat=None)
        await self.send(dict(where, type="session_opened", session=session.session_id, block=block))
        if window_title is not None:
            await window.async_set_title(window_title)
        if tab_title is not None:
            await tab.async_set_title(tab_title)
        return session

    async def resume(self, app, block, chat, folder, window_title, tab_title):
        session = await self.new_tab(app, block, terminal_profile(folder), window_title, tab_title)
        await session.async_set_variable(CHAT_VAR, chat)
        self.sessions[session.session_id]["chat"] = chat
        await session.async_send_text("claude --resume %s\n" % chat)
        return session

    async def op_ping(self, _message):
        return None

    async def op_panel(self, message):
        url = message.get("url")
        need(is_page_url(url), "invalid_url")
        try:
            await iterm2.tool.async_register_web_view_tool(self.connection, PANEL_NAME, PANEL_ID, False, url)
        except Exception:
            await self.send({"type": "panel_failed"})
            raise Refused("panel_failed")
        return None

    async def op_open_chat(self, message):
        chat, sid, folder, block = (message.get(k) for k in ("chat", "session", "folder", "block"))
        need(is_uuid(chat), "invalid_chat")
        need(sid is None or is_uuid(sid), "invalid_session")
        need(is_block(block), "invalid_block")
        window_title = optional_title(message.get("window_title"))
        tab_title = optional_title(message.get("tab_title"))
        app = await self.app()
        async with self.lock:
            session = (app.get_session_by_id(sid) if sid else None) or self.session_for_chat(app, chat)
            if session is None:
                need(is_safe_folder(folder), "invalid_folder")
                session = await self.resume(app, block, chat, folder, window_title, tab_title)
            await self.show(app, session)
            return session.session_id

    async def op_new_chat(self, message):
        block, folder, name = (message.get(k) for k in ("block", "folder", "worktree_name"))
        need(is_block(block), "invalid_block")
        need(is_safe_folder(folder), "invalid_folder")
        need(name is None or is_worktree_name(name), "invalid_worktree_name")
        window_title = optional_title(message.get("window_title"))
        app = await self.app()
        async with self.lock:
            session = await self.new_tab(app, block, terminal_profile(folder), window_title)
            await session.async_send_text("claude --worktree %s\n" % name if name else "claude\n")
            await self.show(app, session)
            return session.session_id

    async def op_open_block(self, message):
        block, chats = message.get("block"), message.get("chats")
        need(is_block(block), "invalid_block")
        need(isinstance(chats, list) and len(chats) <= MAX_ITEMS, "invalid_request")
        need(all(isinstance(item, dict) for item in chats), "invalid_request")
        need(all(is_uuid(item.get("chat")) for item in chats), "invalid_chat")
        window_title = optional_title(message.get("window_title"))
        tab_titles = [optional_title(item.get("tab_title")) for item in chats]
        app = await self.app()
        async with self.lock:
            window = self.block_window(app, block)
            if window is not None:
                await window.async_activate()
                await app.async_activate(raise_all_windows=False)
                return None
            need(chats, "no_chats")
            need(all(is_safe_folder(item.get("folder")) for item in chats), "invalid_folder")
            first = None
            for item, tab_title in zip(chats, tab_titles):
                session = await self.resume(app, block, item["chat"], item["folder"],
                                            window_title if first is None else None, tab_title)
                first = first or session
            await self.show(app, first)
            return first.session_id

    async def op_open_page(self, message):
        url, kind, near = (message.get(k) for k in ("url", "kind", "near_session"))
        need(is_page_url(url), "invalid_url")
        need(kind in ("changes", "board", "probe"), "invalid_kind")
        need(near is None or is_uuid(near), "invalid_session")
        need(self.browser_ok, "browser_unavailable")
        app = await self.app()
        async with self.lock:
            try:
                if kind == "board":
                    return (await self.open_board(app, url)).session_id
                if kind == "probe":
                    return (await self.open_probe(app, url, near)).session_id
                return (await self.open_changes(app, url, near)).session_id
            except Exception:
                raise Refused("browser_unavailable")

    async def open_probe(self, app, url, near):
        """A new browser pane for `blether doctor --live`. Never reused."""
        profile = browser_profile(url)
        near_session = app.get_session_by_id(near) if near else None
        if near_session is not None:
            session = await near_session.async_split_pane(vertical=True, profile_customizations=profile)
        elif app.current_window is not None:
            session = (await app.current_window.async_create_tab(profile_customizations=profile)).sessions[0]
        else:
            window = await iterm2.Window.async_create(self.connection, profile_customizations=profile)
            session = window.tabs[0].sessions[0]
        self.probes.add(session.session_id)
        return session

    async def op_close_session(self, message):
        sid = message.get("session")
        need(is_uuid(sid), "invalid_session")
        need(sid in self.probes, "not a probe session")
        app = await self.app()
        async with self.lock:
            session = app.get_session_by_id(sid)
            if session is not None:
                await session.async_close(force=True)
            self.probes.discard(sid)
        return None

    async def reload(self, session, url):
        """Shows url in an existing browser session, or closes it if that fails."""
        if session is None:
            return None
        try:
            await session.async_load_url(url)
            return session
        except Exception:
            try:
                await session.async_close(force=True)
            except Exception:
                pass
            return None

    async def open_board(self, app, url):
        session = await self.reload(app.get_session_by_id(self.board), url)
        if session is None:
            window = await iterm2.Window.async_create(self.connection, profile_customizations=browser_profile(url))
            session = window.tabs[0].sessions[0]
            self.board = session.session_id
        await self.show(app, session)
        return session

    async def open_changes(self, app, url, near):
        near_session = (app.get_session_by_id(near) if near else None) or current_session(app)
        _window, tab = app.get_window_and_tab_for_session(near_session) if near_session else (None, None)
        need(tab is not None, "browser_unavailable")
        session = await self.reload(app.get_session_by_id(self.splits.get(tab.tab_id)), url)
        if session is None:
            profile = browser_profile(url)
            session = await near_session.async_split_pane(vertical=True, profile_customizations=profile)
            self.splits[tab.tab_id] = session.session_id
        return session

    async def op_set_titles(self, message):
        windows, sessions = message.get("windows") or [], message.get("sessions") or []
        need(isinstance(windows, list) and isinstance(sessions, list), "invalid_request")
        need(len(windows) + len(sessions) <= 1000, "invalid_request")
        need(all(isinstance(item, dict) for item in windows + sessions), "invalid_request")
        need(all(is_block(item.get("block")) for item in windows), "invalid_block")
        need(all(is_uuid(item.get("session")) for item in sessions), "invalid_session")
        window_titles = [(item["block"], clean_title(item.get("title"))) for item in windows]
        tab_titles = [(item["session"], clean_title(item.get("title"))) for item in sessions]
        app = await self.app()
        async with self.lock:
            for block, title in window_titles:
                for window in app.windows:
                    if self.windows.get(window.window_id) == block:
                        await window.async_set_title(title)
            for sid, title in tab_titles:
                session = app.get_session_by_id(sid)
                _window, tab = app.get_window_and_tab_for_session(session) if session else (None, None)
                if tab is not None:
                    await tab.async_set_title(title)
        return None


async def main(connection):
    await Bridge(connection).start()


if __name__ == "__main__":
    iterm2.run_forever(main)

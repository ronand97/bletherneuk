"""A small stand-in for the `iterm2` package.

It covers only what the bridge uses. Every call that would change something in
iTerm2 is recorded in `App.calls`, so tests can check exactly what the bridge
asked for, and that nothing happened when a request was refused.
"""

import asyncio
import itertools
import types


class RPCException(Exception):
    pass


class CreateTabException(Exception):
    pass


class CreateWindowException(Exception):
    pass


class SplitPaneException(Exception):
    pass


class LocalWriteOnlyProfile:
    def __init__(self):
        self.settings = {}

    def _simple_set(self, key, value):
        self.settings[key] = value


class Color:
    def __init__(self, red, green, blue):
        self.red, self.green, self.blue = red, green, blue


class Profile:
    def __init__(self, colors, separate=False):
        self.colors = colors
        self.use_separate_colors_for_light_and_dark_mode = separate

    def get_color_with_key(self, key):
        return self.colors.get(key)

    @staticmethod
    async def async_get_default(connection):
        return connection.app.default_profile


def solid_profile(background, foreground, ansi, suffix=""):
    """A profile whose colours are given as (r, g, b) tuples."""
    colors = {"Background Color" + suffix: Color(*background), "Foreground Color" + suffix: Color(*foreground)}
    for i, rgb in enumerate(ansi):
        colors["Ansi %d Color%s" % (i, suffix)] = Color(*rgb)
    return Profile(colors, separate=bool(suffix))


class Session:
    def __init__(self, app, session_id, settings=None):
        self.app = app
        self.session_id = session_id
        self.settings = settings or {}
        self.variables = {}
        self.profile = app.default_profile

    async def async_send_text(self, text, suppress_broadcast=False):
        self.app.record("send_text", self.session_id, text)

    async def async_activate(self, select_tab=True, order_window_front=True):
        self.app.record("activate_session", self.session_id)

    async def async_get_variable(self, name):
        return self.variables.get(name)

    async def async_set_variable(self, name, value):
        self.app.record("set_session_variable", self.session_id, name, value)
        self.variables[name] = value

    async def async_get_profile(self):
        return self.profile

    async def async_split_pane(self, vertical=False, before=False, profile=None, profile_customizations=None):
        settings = dict(profile_customizations.settings) if profile_customizations else {}
        self.app.record("split_pane", self.session_id, vertical, settings)
        if self.app.fail_browser and settings.get("Custom Command") == "Browser":
            raise SplitPaneException("browser plugin missing")
        new = self.app.new_session(settings)
        _window, tab = self.app.get_window_and_tab_for_session(self)
        tab.sessions.append(new)
        return new

    async def async_load_url(self, url):
        self.app.record("load_url", self.session_id, url)
        if self.app.fail_load_url:
            raise RPCException("load_url refused")

    async def async_close(self, force=False):
        self.app.record("close_session", self.session_id)
        _window, tab = self.app.get_window_and_tab_for_session(self)
        if tab is not None:
            tab.sessions.remove(self)


class Tab:
    def __init__(self, app, tab_id, sessions):
        self.app = app
        self.tab_id = tab_id
        self.sessions = sessions
        self.active_session_id = sessions[0].session_id if sessions else None

    @property
    def all_sessions(self):
        return list(self.sessions)

    @property
    def current_session(self):
        for session in self.sessions:
            if session.session_id == self.active_session_id:
                return session
        return None

    async def async_set_title(self, title):
        self.app.record("set_tab_title", self.tab_id, title)


class Window:
    def __init__(self, app, window_id, tabs):
        self.app = app
        self.window_id = window_id
        self.tabs = tabs
        self.variables = {}
        self.selected_tab_id = tabs[0].tab_id if tabs else None

    @property
    def current_tab(self):
        for tab in self.tabs:
            if tab.tab_id == self.selected_tab_id:
                return tab
        return None

    @staticmethod
    async def async_create(connection, profile=None, command=None, profile_customizations=None):
        app = connection.app
        settings = dict(profile_customizations.settings) if profile_customizations else {}
        app.record("create_window", settings)
        if app.fail_browser and settings.get("Custom Command") == "Browser":
            raise CreateWindowException("browser plugin missing")
        return app.add_window(settings)

    async def async_create_tab(self, profile=None, command=None, index=None, profile_customizations=None, select=True):
        settings = dict(profile_customizations.settings) if profile_customizations else {}
        self.app.record("create_tab", self.window_id, settings)
        if self.app.fail_browser and settings.get("Custom Command") == "Browser":
            raise CreateTabException("browser plugin missing")
        tab =self.app.new_tab([self.app.new_session(settings)])
        self.tabs.append(tab)
        return tab

    async def async_activate(self):
        self.app.record("activate_window", self.window_id)

    async def async_set_title(self, title):
        self.app.record("set_window_title", self.window_id, title)

    async def async_get_variable(self, name):
        return self.variables.get(name)

    async def async_set_variable(self, name, value):
        self.app.record("set_window_variable", self.window_id, name, value)
        self.variables[name] = value


class App:
    def __init__(self):
        self.calls = []
        self.windows = []
        self.current_window = None
        self.default_profile = solid_profile((0, 0, 0), (255, 255, 255), [(i, i, i) for i in range(16)])
        self.variables = {"effectiveTheme": "dark", "appBundlePath": ""}
        self.fail_browser = False
        self.fail_load_url = False
        self.fail_panel = False
        self._ids = itertools.count(1)

    def record(self, *call):
        self.calls.append(call)

    def call_names(self):
        return [call[0] for call in self.calls]

    def new_session(self, settings=None):
        number = next(self._ids)
        return Session(self, "%08X-0000-4000-8000-%012X" % (number, number), settings)

    def new_tab(self, sessions):
        return Tab(self, str(next(self._ids)), sessions)

    def add_window(self, settings=None, sessions=1):
        tab = self.new_tab([self.new_session(settings) for _ in range(sessions)])
        window = Window(self, "pty-window-%d" % next(self._ids), [tab])
        self.windows.append(window)
        if self.current_window is None:
            self.current_window = window
        return window

    def all_sessions(self):
        return [s for w in self.windows for t in w.tabs for s in t.sessions]

    def get_session_by_id(self, session_id, include_buried=True):
        for session in self.all_sessions():
            if session.session_id == session_id:
                return session
        return None

    def get_window_and_tab_for_session(self, session):
        for window in self.windows:
            for tab in window.tabs:
                if session in tab.sessions:
                    return window, tab
        return None, None

    async def async_activate(self, raise_all_windows=True, ignoring_other_apps=False):
        self.record("activate_app")

    async def async_get_variable(self, name):
        return self.variables.get(name)


class Connection:
    def __init__(self, app=None, protocol=(1, 23)):
        self.app = app or App()
        self.iterm2_protocol_version = protocol
        self.rpcs = {}


async def async_get_app(connection, create_if_needed=True):
    return connection.app


def RPC(func):
    async def async_register(connection, timeout=None):
        connection.app.record("register_rpc", func.__name__)
        connection.rpcs[func.__name__] = func

    func.async_register = async_register
    return func


async def async_register_web_view_tool(connection, display_name, identifier, reveal_if_already_registered, url):
    connection.app.record("register_web_view_tool", display_name, identifier, reveal_if_already_registered, url)
    if connection.app.fail_panel:
        raise RPCException("tool registration failed")


tool = types.SimpleNamespace(async_register_web_view_tool=async_register_web_view_tool)


class _Monitor:
    """A monitor whose events tests push with `put`."""

    def __init__(self, connection, *args):
        self.queue = asyncio.Queue()

    async def __aenter__(self):
        return self

    async def __aexit__(self, *exc):
        return False

    async def async_get(self):
        return await self.queue.get()

    async def async_get_next_update(self):
        return await self.queue.get()


class FocusMonitor(_Monitor):
    pass


class NewSessionMonitor(_Monitor):
    pass


class LayoutChangeMonitor(_Monitor):
    pass


class SessionTerminationMonitor(_Monitor):
    pass


class VariableMonitor(_Monitor):
    pass


class VariableScopes:
    SESSION, TAB, WINDOW, APP = range(4)


def run_forever(coro, retry=False, debug=False):
    raise RuntimeError("the fake iterm2 module can't connect to iTerm2")

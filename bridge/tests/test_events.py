"""The hello, the events the bridge reports, the socket link and the open-board RPC."""

import asyncio
import inspect
import json
import os
import plistlib
import shutil
import tempfile

import pytest

import bletherneuk_bridge as bb
import fake_iterm2
from bridge_helpers import CHAT, OTHER_CHAT, block_window, new_bridge, prime, sent


def run(scenario):
    return asyncio.run(scenario())


ANSI = [(i * 10, i * 10 + 1, i * 10 + 2) for i in range(16)]
ANSI_HEX = ["#%02x%02x%02x" % rgb for rgb in ANSI]


def test_hello_shape():
    async def scenario():
        bridge = new_bridge()
        bridge.iterm_version = "3.7.6"
        app = bridge.connection.app
        app.default_profile = fake_iterm2.solid_profile((36, 39, 58), (202, 211, 245), ANSI)
        first = block_window(app, 3, sessions=2)
        first.tabs[0].sessions[0].variables["user.bletherneuk_chat"] = CHAT
        first.tabs[0].sessions[1].variables["user.bletherneuk_chat"] = "not a uuid"
        second = app.add_window()
        s1, s2 = first.tabs[0].sessions
        s3 = second.tabs[0].sessions[0]
        async with bridge.lock:
            hello = await bridge.hello(app)
        assert hello == {
            "type": "bridge_hello",
            "bridge_version": "0.1.0",
            "iterm_version": "3.7.6",
            "colors": {"background": "#24273a", "foreground": "#cad3f5", "ansi": ANSI_HEX},
            "sessions": [
                {"session": s1.session_id, "window": first.window_id, "tab": first.tabs[0].tab_id, "block": 3, "chat": CHAT},
                {"session": s2.session_id, "window": first.window_id, "tab": first.tabs[0].tab_id, "block": 3, "chat": None},
                {"session": s3.session_id, "window": second.window_id, "tab": second.tabs[0].tab_id, "block": None, "chat": None},
            ],
            "focus": {"session": s1.session_id, "window": first.window_id},
        }
        assert app.calls == []  # the hello only reads
        json.dumps(hello)

    run(scenario)


def test_hello_with_no_windows():
    async def scenario():
        bridge = new_bridge()
        async with bridge.lock:
            hello = await bridge.hello(bridge.connection.app)
        assert hello["sessions"] == [] and hello["focus"] == {"session": None, "window": None}
        assert hello["colors"]["ansi"] == ["#%02x%02x%02x" % (i, i, i) for i in range(16)]  # the default profile

    run(scenario)


@pytest.mark.parametrize("theme, suffix", [("dark", " (Dark)"), ("light highContrast", " (Light)")])
def test_colours_follow_the_light_or_dark_variant(theme, suffix):
    async def scenario():
        bridge = new_bridge()
        app = bridge.connection.app
        app.variables["effectiveTheme"] = theme
        app.default_profile = fake_iterm2.solid_profile((1, 2, 3), (4, 5, 6), ANSI, suffix=suffix)
        colors = await bridge.read_colors(app)
        assert colors == {"background": "#010203", "foreground": "#040506", "ansi": ANSI_HEX}

    run(scenario)


def test_colours_are_null_when_the_profile_is_incomplete():
    async def scenario():
        bridge = new_bridge()
        app = bridge.connection.app
        app.default_profile = fake_iterm2.solid_profile((1, 2, 3), (4, 5, 6), ANSI[:15])
        assert await bridge.read_colors(app) is None

    run(scenario)


def test_colour_channels_are_rounded_and_clamped():
    assert bb.hex_color(fake_iterm2.Color(254.6, -3, 300)) == "#ff00ff"


def test_a_new_session_is_reported():
    async def scenario():
        bridge = new_bridge()
        app = bridge.connection.app
        await prime(bridge)
        window = app.add_window()
        await bridge.on_layout(None)
        await bridge.on_layout(None)  # reported once only
        session = window.tabs[0].sessions[0]
        assert sent(bridge) == [{"type": "session_opened", "session": session.session_id,
                                 "window": window.window_id, "tab": window.tabs[0].tab_id, "block": None}]

    run(scenario)


def test_a_closed_window_is_reported_with_its_chats():
    async def scenario():
        bridge = new_bridge()
        app = bridge.connection.app
        window = block_window(app, 3, sessions=3)
        window.tabs[0].sessions[0].variables["user.bletherneuk_chat"] = CHAT
        window.tabs[0].sessions[1].variables["user.bletherneuk_chat"] = OTHER_CHAT
        app.add_window()
        await prime(bridge)
        app.windows.remove(window)
        await bridge.on_layout(None)
        assert sent(bridge) == [{"type": "window_closed", "window": window.window_id, "block": 3,
                                 "chats": [CHAT, OTHER_CHAT]}]

    run(scenario)


def test_a_session_ending_is_reported_after_its_window():
    async def scenario():
        bridge = new_bridge()
        app = bridge.connection.app
        window = block_window(app, 3)
        session = window.tabs[0].sessions[0]
        session.variables["user.bletherneuk_chat"] = CHAT
        await prime(bridge)
        app.windows.remove(window)
        await bridge.on_terminated(session.session_id)
        assert [m["type"] for m in sent(bridge)] == ["window_closed", "session_closed"]
        assert sent(bridge)[0]["chats"] == [CHAT]
        assert sent(bridge)[1] == {"type": "session_closed", "session": session.session_id}

    run(scenario)


def test_focus_is_reported_when_it_changes():
    async def scenario():
        bridge = new_bridge()
        app = bridge.connection.app
        app.add_window()
        other = app.add_window()
        await prime(bridge)
        await bridge.on_focus(None)
        assert sent(bridge, "focus") == []
        app.current_window = other
        await bridge.on_focus(None)
        await bridge.on_focus(None)
        assert sent(bridge, "focus") == [{"type": "focus", "session": other.tabs[0].sessions[0].session_id,
                                          "window": other.window_id}]

    run(scenario)


def test_colours_are_reported_when_they_change():
    async def scenario():
        bridge = new_bridge()
        app = bridge.connection.app
        session = app.add_window().tabs[0].sessions[0]
        await prime(bridge)
        await bridge.on_theme("dark")
        assert sent(bridge, "colors") == []
        session.profile = fake_iterm2.solid_profile((1, 2, 3), (4, 5, 6), ANSI)
        await bridge.on_theme("dark")
        assert sent(bridge, "colors") == [{"type": "colors", "colors": {
            "background": "#010203", "foreground": "#040506", "ansi": ANSI_HEX}}]

    run(scenario)


def test_events_are_dropped_while_the_service_is_away():
    async def scenario():
        bridge = new_bridge()
        bridge.writer = None
        await bridge.send({"type": "focus", "session": None, "window": None})

    run(scenario)


# Real Unix sockets ------------------------------------------------------------


@pytest.fixture
def socket_dir():
    # A short folder: socket paths are limited to 104 bytes.
    path = tempfile.mkdtemp(prefix="bn")
    yield path
    shutil.rmtree(path, ignore_errors=True)


def test_link_sends_hello_first_then_answers_requests(socket_dir):
    async def scenario():
        path = os.path.join(socket_dir, "s.sock")
        seen = asyncio.get_running_loop().create_future()

        async def service(reader, writer):
            hello = json.loads(await reader.readline())
            writer.write(b'{"type":"request","id":1,"op":"ping"}\n')
            writer.write(b"not json\n")
            writer.write(b'{"type":"request","id":2,"op":"open_chat","chat":"x"}\n')
            await writer.drain()
            replies = [json.loads(await reader.readline()) for _ in range(2)]
            seen.set_result((hello, replies))
            writer.close()

        server = await asyncio.start_unix_server(service, path=path)
        bridge = new_bridge()
        bridge.path = path
        bridge.writer = None
        block_window(bridge.connection.app, 3)
        task = asyncio.ensure_future(bridge.run_link())
        try:
            hello, replies = await asyncio.wait_for(seen, 5)
        finally:
            task.cancel()
            server.close()
        assert hello["type"] == "bridge_hello" and len(hello["sessions"]) == 1
        assert replies == [
            {"type": "reply", "id": 1, "ok": True, "error": None, "session": None},
            {"type": "reply", "id": 2, "ok": False, "error": "invalid_chat", "session": None},
        ]

    run(scenario)


def test_link_refuses_a_path_that_is_not_a_socket(socket_dir):
    async def scenario():
        path = os.path.join(socket_dir, "s.sock")
        with open(path, "w") as handle:
            handle.write("x")
        bridge = new_bridge()
        bridge.path = path
        with pytest.raises(OSError):
            await bridge.open_socket()
        bridge.path = os.path.join(socket_dir, "missing.sock")
        with pytest.raises(OSError):
            await bridge.open_socket()

    run(scenario)


def test_open_board_rpc_takes_no_arguments_and_asks_the_service(socket_dir):
    async def scenario():
        path = os.path.join(socket_dir, "s.sock")
        received = asyncio.get_running_loop().create_future()

        async def service(reader, writer):
            received.set_result(json.loads(await reader.readline()))
            writer.write(b'{"ok":true,"via":"iterm2"}\n')
            await writer.drain()
            writer.close()

        server = await asyncio.start_unix_server(service, path=path)
        bridge = new_bridge()
        bridge.path = path
        await bridge.register_rpc()
        rpc = bridge.connection.rpcs["bletherneuk_open_board"]
        assert list(inspect.signature(rpc).parameters) == []
        try:
            await rpc()
            message = await asyncio.wait_for(received, 5)
            await asyncio.wait_for(asyncio.gather(*bridge.tasks), 5)
        finally:
            server.close()
        assert message == {"type": "open_page", "page": "board", "block": None, "near_session": None}

    run(scenario)


def test_start_reads_the_iterm2_version_and_registers_everything(tmp_path):
    async def scenario():
        bundle = tmp_path / "iTerm.app"
        (bundle / "Contents").mkdir(parents=True)
        with open(bundle / "Contents" / "Info.plist", "wb") as handle:
            plistlib.dump({"CFBundleShortVersionString": "3.7.6"}, handle)
        bridge = new_bridge(protocol=(1, 0))
        bridge.browser_ok = False
        bridge.connection.app.variables["appBundlePath"] = str(bundle)
        await bridge.start()
        try:
            assert bridge.iterm_version == "3.7.6" and bridge.browser_ok is True
            assert bridge.connection.app.calls == [("register_rpc", "bletherneuk_open_board")]
            assert len(bridge.tasks) == 6
        finally:
            for task in list(bridge.tasks):
                task.cancel()
            await asyncio.gather(*bridge.tasks, return_exceptions=True)

    run(scenario)


def test_start_copes_without_a_version(tmp_path):
    async def scenario():
        bridge = new_bridge(protocol=(1, 22))
        bridge.connection.app.variables["appBundlePath"] = ""
        await bridge.start()
        try:
            assert bridge.iterm_version == "unknown" and bridge.browser_ok is False
        finally:
            for task in list(bridge.tasks):
                task.cancel()
            await asyncio.gather(*bridge.tasks, return_exceptions=True)

    run(scenario)

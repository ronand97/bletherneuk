"""Each service request makes the right iTerm2 calls, and refused requests make none."""

import asyncio
import re

import pytest

import bletherneuk_bridge as bb
import fake_iterm2
from bridge_helpers import CHAT, OTHER_CHAT, URL, block_window, new_bridge, prime, request, sent

TYPED = re.compile(r"claude(?: --resume [0-9a-f-]{36}| --worktree [A-Za-z0-9._-]+)?\n")


@pytest.fixture
def folder(tmp_path):
    path = tmp_path / "acme" / "shop"
    path.mkdir(parents=True)
    return str(path)


def run(scenario):
    return asyncio.run(scenario())


def test_ping_replies():
    async def scenario():
        bridge = new_bridge()
        reply = await request(bridge, "ping")
        assert reply == {"type": "reply", "id": 7, "ok": True, "error": None, "session": None}

    run(scenario)


def test_unknown_op_is_refused():
    async def scenario():
        bridge = new_bridge()
        reply = await request(bridge, "run_shell", command="ls")
        assert reply["ok"] is False and reply["error"] == "unknown_op"
        assert bridge.connection.app.calls == []

    run(scenario)


@pytest.mark.parametrize("request_id", [None, "7", True, 1.5])
def test_request_without_an_integer_id_gets_no_reply(request_id):
    async def scenario():
        bridge = new_bridge()
        await bridge.handle({"type": "request", "id": request_id, "op": "ping"})
        assert sent(bridge) == []

    run(scenario)


def test_open_chat_opens_a_window_for_the_block(folder):
    async def scenario():
        bridge = new_bridge()
        app = bridge.connection.app
        reply = await request(bridge, "open_chat", chat=CHAT, session=None, folder=folder, block=3,
                              window_title="Retry backoff", tab_title="Add jitter")
        window = app.windows[0]
        session = window.tabs[0].sessions[0]
        assert reply["ok"] is True and reply["session"] == session.session_id
        assert app.calls == [
            ("create_window", {"Custom Directory": "Yes", "Working Directory": folder}),
            ("set_window_variable", window.window_id, "user.bletherneuk_block", 3),
            ("set_window_title", window.window_id, "Retry backoff"),
            ("set_tab_title", window.tabs[0].tab_id, "Add jitter"),
            ("set_session_variable", session.session_id, "user.bletherneuk_chat", CHAT),
            ("send_text", session.session_id, "claude --resume %s\n" % CHAT),
            ("activate_session", session.session_id),
            ("activate_app",),
        ]
        assert sent(bridge, "session_opened") == [{
            "type": "session_opened", "session": session.session_id, "window": window.window_id,
            "tab": window.tabs[0].tab_id, "block": 3,
        }]

    run(scenario)


def test_open_chat_adds_a_tab_to_the_blocks_window(folder):
    async def scenario():
        bridge = new_bridge()
        app = bridge.connection.app
        app.add_window()  # another window, not this block's
        window = block_window(app, 3)
        await prime(bridge)
        reply = await request(bridge, "open_chat", chat=CHAT, session=None, folder=folder, block=3)
        assert reply["ok"] is True
        assert app.call_names()[0] == "create_tab"
        assert app.calls[0] == ("create_tab", window.window_id, {"Custom Directory": "Yes", "Working Directory": folder})
        assert len(window.tabs) == 2 and reply["session"] == window.tabs[1].sessions[0].session_id

    run(scenario)


def test_open_chat_focuses_the_session_when_it_is_open():
    async def scenario():
        bridge = new_bridge()
        app = bridge.connection.app
        session = block_window(app, 3).tabs[0].sessions[0]
        reply = await request(bridge, "open_chat", chat=CHAT, session=session.session_id,
                              folder="/Users/someone/repos/gone", block=3)
        assert reply["ok"] is True and reply["session"] == session.session_id
        assert app.calls == [("activate_session", session.session_id), ("activate_app",)]

    run(scenario)


def test_open_chat_finds_an_open_session_by_its_chat_variable(folder):
    async def scenario():
        bridge = new_bridge()
        app = bridge.connection.app
        session = block_window(app, 3).tabs[0].sessions[0]
        session.variables["user.bletherneuk_chat"] = CHAT
        await prime(bridge)
        reply = await request(bridge, "open_chat", chat=CHAT, session=None, folder=folder, block=3)
        assert reply["session"] == session.session_id
        assert "send_text" not in app.call_names()

    run(scenario)


def test_new_chat_runs_claude(folder):
    async def scenario():
        bridge = new_bridge()
        app = bridge.connection.app
        reply = await request(bridge, "new_chat", block=5, folder=folder, worktree_name=None, window_title="Shop")
        session = app.windows[0].tabs[0].sessions[0]
        assert reply["ok"] is True and reply["session"] == session.session_id
        assert ("send_text", session.session_id, "claude\n") in app.calls
        assert ("set_window_variable", app.windows[0].window_id, "user.bletherneuk_block", 5) in app.calls
        assert sent(bridge, "session_opened")[0]["block"] == 5

    run(scenario)


def test_new_chat_in_a_new_worktree(folder):
    async def scenario():
        bridge = new_bridge()
        app = bridge.connection.app
        await request(bridge, "new_chat", block=5, folder=folder, worktree_name="retry-backoff")
        session = app.windows[0].tabs[0].sessions[0]
        assert ("send_text", session.session_id, "claude --worktree retry-backoff\n") in app.calls

    run(scenario)


@pytest.mark.parametrize("name", ["--dangerously-skip-permissions", "retry backoff", "x;rm -rf ~", "a" * 65, ""])
def test_new_chat_refuses_a_bad_worktree_name(folder, name):
    async def scenario():
        bridge = new_bridge()
        reply = await request(bridge, "new_chat", block=5, folder=folder, worktree_name=name)
        assert reply["error"] == "invalid_worktree_name"
        assert bridge.connection.app.calls == []

    run(scenario)


def test_open_block_opens_a_tab_per_chat(tmp_path):
    async def scenario():
        first, second = tmp_path / "shop", tmp_path / "shop-retry"
        first.mkdir()
        second.mkdir()
        bridge = new_bridge()
        app = bridge.connection.app
        reply = await request(bridge, "open_block", block=3, window_title="Retry backoff", chats=[
            {"chat": CHAT, "folder": str(first), "tab_title": "Plan"},
            {"chat": OTHER_CHAT, "folder": str(second), "tab_title": "Build"},
        ])
        window = app.windows[0]
        sessions = [tab.sessions[0] for tab in window.tabs]
        assert reply["ok"] is True and reply["session"] == sessions[0].session_id
        assert app.call_names().count("create_window") == 1
        assert app.calls[0] == ("create_window", {"Custom Directory": "Yes", "Working Directory": str(first)})
        assert ("create_tab", window.window_id, {"Custom Directory": "Yes", "Working Directory": str(second)}) in app.calls
        assert app.call_names().count("set_window_title") == 1
        assert ("send_text", sessions[0].session_id, "claude --resume %s\n" % CHAT) in app.calls
        assert ("send_text", sessions[1].session_id, "claude --resume %s\n" % OTHER_CHAT) in app.calls
        assert ("set_tab_title", window.tabs[1].tab_id, "Build") in app.calls

    run(scenario)


def test_open_block_focuses_the_open_window():
    async def scenario():
        bridge = new_bridge()
        app = bridge.connection.app
        window = block_window(app, 3)
        await prime(bridge)
        reply = await request(bridge, "open_block", block=3, window_title="Retry backoff", chats=[
            {"chat": CHAT, "folder": "/Users/someone/repos/gone", "tab_title": "Plan"},
        ])
        assert reply["ok"] is True
        assert app.calls == [("activate_window", window.window_id), ("activate_app",)]

    run(scenario)


def test_open_block_refuses_a_bad_folder_before_opening_anything(folder):
    async def scenario():
        bridge = new_bridge()
        reply = await request(bridge, "open_block", block=3, window_title="x", chats=[
            {"chat": CHAT, "folder": folder, "tab_title": "Plan"},
            {"chat": OTHER_CHAT, "folder": "relative/shop", "tab_title": "Build"},
        ])
        assert reply["error"] == "invalid_folder"
        assert bridge.connection.app.calls == []

    run(scenario)


def test_folders_only_travel_through_profile_settings(folder):
    async def scenario():
        bridge = new_bridge()
        app = bridge.connection.app
        await request(bridge, "open_chat", chat=CHAT, session=None, folder=folder, block=1, window_title="a", tab_title="b")
        await request(bridge, "new_chat", block=2, folder=folder, worktree_name="retry-backoff", window_title="c")
        await request(bridge, "new_chat", block=2, folder=folder, worktree_name=None, window_title="c")
        await request(bridge, "open_block", block=3, window_title="d", chats=[{"chat": OTHER_CHAT, "folder": folder, "tab_title": "e"}])
        typed = [call[2] for call in app.calls if call[0] == "send_text"]
        assert len(typed) == 4
        for text in typed:
            assert folder not in text
            assert TYPED.fullmatch(text)
        created = [call[-1] for call in app.calls if call[0] in ("create_window", "create_tab")]
        assert created and all(settings == {"Custom Directory": "Yes", "Working Directory": folder} for settings in created)

    run(scenario)


@pytest.mark.parametrize("chat", [CHAT + "\n", "not-a-uuid", "{%s}" % CHAT, CHAT + "; rm -rf ~", 42, None])
def test_chat_ids_that_are_not_uuids_are_refused(folder, chat):
    async def scenario():
        bridge = new_bridge()
        block_window(bridge.connection.app, 3)
        await prime(bridge)
        replies = [
            await request(bridge, "open_chat", chat=chat, session=None, folder=folder, block=3),
            await request(bridge, "open_block", block=4, window_title="x", chats=[{"chat": chat, "folder": folder}]),
        ]
        assert [reply["error"] for reply in replies] == ["invalid_chat", "invalid_chat"]
        assert bridge.connection.app.calls == []

    run(scenario)


def test_session_ids_must_be_uuids(folder):
    async def scenario():
        bridge = new_bridge()
        reply = await request(bridge, "open_chat", chat=CHAT, session=CHAT + "\n", folder=folder, block=3)
        assert reply["error"] == "invalid_session"
        reply = await request(bridge, "open_page", url=URL, kind="changes", near_session="w0t0p0")
        assert reply["error"] == "invalid_session"
        assert bridge.connection.app.calls == []

    run(scenario)


@pytest.mark.parametrize("block", ["3", -1, True, None, 2.5])
def test_block_ids_must_be_integers(folder, block):
    async def scenario():
        bridge = new_bridge()
        reply = await request(bridge, "new_chat", block=block, folder=folder, worktree_name=None)
        assert reply["error"] == "invalid_block"
        assert bridge.connection.app.calls == []

    run(scenario)


@pytest.mark.parametrize("name", ["shop\x1b]0;x\x07", "shop\nclaude --dangerously-skip-permissions", "shop\u202egnp"])
def test_a_folder_with_control_characters_is_refused(tmp_path, name):
    async def scenario():
        bad = tmp_path / name
        bad.mkdir()
        bridge = new_bridge()
        replies = [
            await request(bridge, "open_chat", chat=CHAT, session=None, folder=str(bad), block=3),
            await request(bridge, "new_chat", block=3, folder=str(bad), worktree_name=None),
            await request(bridge, "open_block", block=3, window_title="x", chats=[{"chat": CHAT, "folder": str(bad)}]),
        ]
        assert [reply["error"] for reply in replies] == ["invalid_folder"] * 3
        assert bridge.connection.app.calls == []

    run(scenario)


def test_a_title_containing_an_interpolation_is_escaped(folder):
    async def scenario():
        bridge = new_bridge()
        app = bridge.connection.app
        evil = '\\(iterm2.alert(title: "x", subtitle: "y", buttons: ["OK"]))'
        escaped = '\\\\(iterm2.alert(title: "x", subtitle: "y", buttons: ["OK"]))'
        await request(bridge, "open_chat", chat=CHAT, session=None, folder=folder, block=3,
                      window_title=evil, tab_title="Fix " + evil)
        window = app.windows[0]
        session = window.tabs[0].sessions[0]
        reply = await request(bridge, "set_titles", windows=[{"block": 3, "title": evil}],
                              sessions=[{"session": session.session_id, "title": evil + "\n"}])
        assert reply["ok"] is True
        titles = [call[2] for call in app.calls if call[0] in ("set_window_title", "set_tab_title")]
        assert titles == [escaped, "Fix " + escaped, escaped, escaped]

    run(scenario)


def test_set_titles_names_windows_by_block_and_tabs_by_session():
    async def scenario():
        bridge = new_bridge()
        app = bridge.connection.app
        window = block_window(app, 3)
        other = block_window(app, 4)
        await prime(bridge)
        session = other.tabs[0].sessions[0]
        reply = await request(bridge, "set_titles", windows=[{"block": 3, "title": "Retry backoff"}],
                              sessions=[{"session": session.session_id, "title": "Add jitter"},
                                        {"session": OTHER_CHAT, "title": "not open"}])
        assert reply["ok"] is True
        assert app.calls == [("set_window_title", window.window_id, "Retry backoff"),
                             ("set_tab_title", other.tabs[0].tab_id, "Add jitter")]

    run(scenario)


@pytest.mark.parametrize("fields", [
    {"windows": [{"block": "3", "title": "x"}]},
    {"sessions": [{"session": "w0t0p0", "title": "x"}]},
    {"windows": [{"block": 3, "title": 7}]},
    {"windows": "all"},
])
def test_set_titles_refuses_malformed_entries(fields):
    async def scenario():
        bridge = new_bridge()
        block_window(bridge.connection.app, 3)
        await prime(bridge)
        reply = await request(bridge, "set_titles", **fields)
        assert reply["ok"] is False
        assert bridge.connection.app.calls == []

    run(scenario)


def test_panel_registers_the_toolbelt_web_view():
    async def scenario():
        bridge = new_bridge()
        reply = await request(bridge, "panel", url=URL)
        assert reply["ok"] is True
        assert bridge.connection.app.calls == [("register_web_view_tool", "bletherneuk", "dev.bletherneuk.panel", False, URL)]

    run(scenario)


def test_panel_failure_is_reported():
    async def scenario():
        bridge = new_bridge()
        bridge.connection.app.fail_panel = True
        reply = await request(bridge, "panel", url=URL)
        assert reply["ok"] is False and reply["error"] == "panel_failed"
        assert {"type": "panel_failed"} in sent(bridge)

    run(scenario)


@pytest.mark.parametrize("url", ["https://127.0.0.1:49152/", "http://localhost:49152/", "http://127.0.0.1:49152@evil.example/",
                                 URL + "\n", "javascript:alert(1)", "file:///etc/passwd"])
def test_page_urls_must_be_local_http(url):
    async def scenario():
        bridge = new_bridge()
        session = bridge.connection.app.add_window().tabs[0].sessions[0]
        replies = [
            await request(bridge, "panel", url=url),
            await request(bridge, "open_page", url=url, kind="board", near_session=None),
            await request(bridge, "open_page", url=url, kind="changes", near_session=session.session_id),
        ]
        assert [reply["error"] for reply in replies] == ["invalid_url"] * 3
        assert bridge.connection.app.calls == []

    run(scenario)


def test_open_page_changes_splits_the_session_with_a_browser():
    async def scenario():
        bridge = new_bridge()
        app = bridge.connection.app
        session = app.add_window().tabs[0].sessions[0]
        reply = await request(bridge, "open_page", url=URL, kind="changes", near_session=session.session_id)
        split = app.windows[0].tabs[0].sessions[1]
        assert reply["ok"] is True and reply["session"] == split.session_id
        assert app.calls == [("split_pane", session.session_id, True, {"Custom Command": "Browser", "Initial URL": URL})]

    run(scenario)


@pytest.mark.parametrize("near", [None, OTHER_CHAT])
def test_open_page_changes_uses_the_focused_session_when_none_is_known(near):
    async def scenario():
        bridge = new_bridge()
        app = bridge.connection.app
        session = app.add_window().tabs[0].sessions[0]
        reply = await request(bridge, "open_page", url=URL, kind="changes", near_session=near)
        assert reply["ok"] is True
        assert app.call_names() == ["split_pane"] and app.calls[0][1] == session.session_id

    run(scenario)


def test_open_page_changes_reuses_its_split():
    async def scenario():
        bridge = new_bridge()
        app = bridge.connection.app
        session = app.add_window().tabs[0].sessions[0]
        first = await request(bridge, "open_page", url=URL, kind="changes", near_session=session.session_id)
        second_url = URL.replace("block=3", "block=4")
        second = await request(bridge, "open_page", url=second_url, kind="changes", near_session=session.session_id)
        assert second["session"] == first["session"]
        assert app.call_names() == ["split_pane", "load_url"]
        assert app.calls[1] == ("load_url", first["session"], second_url)

    run(scenario)


def test_open_page_changes_replaces_its_split_when_it_cannot_load_a_url():
    async def scenario():
        bridge = new_bridge()
        app = bridge.connection.app
        session = app.add_window().tabs[0].sessions[0]
        first = await request(bridge, "open_page", url=URL, kind="changes", near_session=session.session_id)
        app.fail_load_url = True
        second = await request(bridge, "open_page", url=URL, kind="changes", near_session=session.session_id)
        assert second["ok"] is True and second["session"] != first["session"]
        assert app.call_names() == ["split_pane", "load_url", "close_session", "split_pane"]
        assert len(app.windows[0].tabs[0].sessions) == 2

    run(scenario)


def test_open_page_board_opens_its_own_window_then_reuses_it():
    async def scenario():
        bridge = new_bridge()
        app = bridge.connection.app
        first = await request(bridge, "open_page", url=URL, kind="board", near_session=None)
        board = app.windows[0].tabs[0].sessions[0]
        assert first["ok"] is True and first["session"] == board.session_id
        assert app.calls[0] == ("create_window", {"Custom Command": "Browser", "Initial URL": URL})
        second = await request(bridge, "open_page", url=URL, kind="board", near_session=None)
        assert second["session"] == board.session_id
        assert app.call_names() == ["create_window", "activate_session", "activate_app",
                                    "load_url", "activate_session", "activate_app"]

    run(scenario)


@pytest.mark.parametrize("kind", ["changes", "board"])
def test_browser_fallback_when_a_browser_session_cannot_be_created(kind):
    async def scenario():
        bridge = new_bridge()
        app = bridge.connection.app
        session = app.add_window().tabs[0].sessions[0]
        app.fail_browser = True
        reply = await request(bridge, "open_page", url=URL, kind=kind, near_session=session.session_id)
        assert reply == {"type": "reply", "id": 7, "ok": False, "error": "browser_unavailable", "session": None}

    run(scenario)


def test_browser_fallback_when_there_is_no_session_to_split():
    async def scenario():
        bridge = new_bridge()
        reply = await request(bridge, "open_page", url=URL, kind="changes", near_session=None)
        assert reply["error"] == "browser_unavailable"
        assert bridge.connection.app.calls == []

    run(scenario)


def test_browser_fallback_on_iterm2_older_than_3_7_6():
    async def scenario():
        bridge = new_bridge()
        bridge.browser_ok = False
        session = bridge.connection.app.add_window().tabs[0].sessions[0]
        reply = await request(bridge, "open_page", url=URL, kind="changes", near_session=session.session_id)
        assert reply["error"] == "browser_unavailable"
        assert bridge.connection.app.calls == []

    run(scenario)


def test_an_iterm2_error_is_reported_not_raised(folder):
    async def scenario():
        bridge = new_bridge()

        async def broken(*args, **kwargs):
            raise fake_iterm2.CreateWindowException("no")

        fake_iterm2.Window.async_create, original = broken, fake_iterm2.Window.async_create
        try:
            reply = await request(bridge, "new_chat", block=3, folder=folder, worktree_name=None)
        finally:
            fake_iterm2.Window.async_create = original
        assert reply["ok"] is False and reply["error"] == "iterm2_error"

    run(scenario)


def test_refusals_happen_before_iterm2_is_asked_anything(folder, monkeypatch):
    async def scenario():
        bridge = new_bridge()
        asked = []

        async def get_app(connection, create_if_needed=True):
            asked.append(True)
            return connection.app

        monkeypatch.setattr(bb.iterm2, "async_get_app", get_app)
        await request(bridge, "open_chat", chat=CHAT + "\n", session=None, folder=folder, block=3)
        await request(bridge, "new_chat", block=3, folder="relative", worktree_name=None)
        await request(bridge, "open_page", url="https://example.com/", kind="board", near_session=None)
        assert asked == []

    run(scenario)


BROWSER = {"Custom Command": "Browser", "Initial URL": URL}


def test_probe_opens_a_new_split_every_time():
    async def scenario():
        bridge = new_bridge()
        app = bridge.connection.app
        session = app.add_window().tabs[0].sessions[0]
        first = await request(bridge, "open_page", url=URL, kind="probe", near_session=session.session_id)
        second = await request(bridge, "open_page", url=URL, kind="probe", near_session=session.session_id)
        assert first["ok"] is True and second["ok"] is True
        assert first["session"] != second["session"]
        assert app.calls == [("split_pane", session.session_id, True, BROWSER)] * 2
        assert [s.session_id for s in app.windows[0].tabs[0].sessions[1:]] == [first["session"], second["session"]]

    run(scenario)


def test_probe_never_reuses_the_changes_split():
    async def scenario():
        bridge = new_bridge()
        app = bridge.connection.app
        session = app.add_window().tabs[0].sessions[0]
        changes = await request(bridge, "open_page", url=URL, kind="changes", near_session=session.session_id)
        probe = await request(bridge, "open_page", url=URL, kind="probe", near_session=session.session_id)
        assert probe["session"] != changes["session"]
        assert app.call_names() == ["split_pane", "split_pane"]

    run(scenario)


@pytest.mark.parametrize("near", [None, OTHER_CHAT])
def test_probe_without_a_known_session_opens_a_tab_in_the_current_window(near):
    async def scenario():
        bridge = new_bridge()
        app = bridge.connection.app
        app.add_window()
        window = app.add_window()
        app.current_window = window
        reply = await request(bridge, "open_page", url=URL, kind="probe", near_session=near)
        assert app.calls == [("create_tab", window.window_id, BROWSER)]
        assert reply["ok"] is True and reply["session"] == window.tabs[1].sessions[0].session_id

    run(scenario)


def test_probe_with_no_windows_opens_one():
    async def scenario():
        bridge = new_bridge()
        app = bridge.connection.app
        reply = await request(bridge, "open_page", url=URL, kind="probe", near_session=None)
        assert app.calls == [("create_window", BROWSER)]
        assert reply["session"] == app.windows[0].tabs[0].sessions[0].session_id

    run(scenario)


@pytest.mark.parametrize("with_session", [True, False])
def test_probe_falls_back_when_a_browser_session_cannot_be_created(with_session):
    async def scenario():
        bridge = new_bridge()
        app = bridge.connection.app
        session = app.add_window().tabs[0].sessions[0]
        app.fail_browser = True
        near = session.session_id if with_session else None
        reply = await request(bridge, "open_page", url=URL, kind="probe", near_session=near)
        assert reply == {"type": "reply", "id": 7, "ok": False, "error": "browser_unavailable", "session": None}
        assert bridge.probes == set()

    run(scenario)


def test_probe_urls_are_validated():
    async def scenario():
        bridge = new_bridge()
        session = bridge.connection.app.add_window().tabs[0].sessions[0]
        reply = await request(bridge, "open_page", url="https://example.com/", kind="probe", near_session=session.session_id)
        assert reply["error"] == "invalid_url"
        assert bridge.connection.app.calls == []

    run(scenario)


def test_close_session_closes_a_probe():
    async def scenario():
        bridge = new_bridge()
        app = bridge.connection.app
        session = app.add_window().tabs[0].sessions[0]
        probe = await request(bridge, "open_page", url=URL, kind="probe", near_session=session.session_id)
        reply = await request(bridge, "close_session", session=probe["session"])
        assert reply["ok"] is True
        assert app.calls[-1] == ("close_session", probe["session"])
        assert [s.session_id for s in app.windows[0].tabs[0].sessions] == [session.session_id]
        again = await request(bridge, "close_session", session=probe["session"])
        assert again["error"] == "not a probe session"

    run(scenario)


def test_close_session_refuses_sessions_that_are_not_probes():
    async def scenario():
        bridge = new_bridge()
        app = bridge.connection.app
        session = app.add_window().tabs[0].sessions[0]
        changes = await request(bridge, "open_page", url=URL, kind="changes", near_session=session.session_id)
        calls = list(app.calls)
        for target in (session.session_id, changes["session"]):
            reply = await request(bridge, "close_session", session=target)
            assert reply["ok"] is False and reply["error"] == "not a probe session"
        assert app.calls == calls

    run(scenario)


@pytest.mark.parametrize("target", [None, "w0t0p0", 42])
def test_close_session_needs_a_uuid(target):
    async def scenario():
        bridge = new_bridge()
        reply = await request(bridge, "close_session", session=target)
        assert reply["error"] == "invalid_session"

    run(scenario)


def test_close_session_refuses_a_probe_id_with_a_trailing_newline():
    async def scenario():
        bridge = new_bridge()
        app = bridge.connection.app
        session = app.add_window().tabs[0].sessions[0]
        probe = await request(bridge, "open_page", url=URL, kind="probe", near_session=session.session_id)
        reply = await request(bridge, "close_session", session=probe["session"] + "\n")
        assert reply["error"] == "invalid_session"
        assert app.call_names() == ["split_pane"]

    run(scenario)

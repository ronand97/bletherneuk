"""Shared helpers for the bridge tests."""

import json

import bletherneuk_bridge
import fake_iterm2

CHAT = "123e4567-e89b-42d3-a456-426614174000"
OTHER_CHAT = "9b2f6c1e-4d3a-4f5b-8c7d-0e1f2a3b4c5d"
URL = "http://127.0.0.1:49152/auth?code=abc123&next=/changes?block=3"


class FakeWriter:
    """Collects the lines the bridge sends to the service."""

    def __init__(self):
        self.data = b""
        self.closed = False

    def write(self, data):
        self.data += data

    async def drain(self):
        pass

    def close(self):
        self.closed = True

    def messages(self):
        return [json.loads(line) for line in self.data.decode().splitlines()]


def new_bridge(app=None, protocol=(1, 23)):
    """A bridge on a fake iTerm2, sending to a FakeWriter. Call inside a running loop."""
    bridge = bletherneuk_bridge.Bridge(fake_iterm2.Connection(app, protocol), path="/nonexistent/blether.sock")
    bridge.writer = FakeWriter()
    bridge.browser_ok = True
    return bridge


def sent(bridge, kind=None):
    return [m for m in bridge.writer.messages() if kind is None or m["type"] == kind]


async def prime(bridge):
    """Runs the hello so the bridge knows the current windows, then forgets what it sent."""
    async with bridge.lock:
        await bridge.hello(bridge.connection.app)
    bridge.writer.data = b""


async def request(bridge, op, **fields):
    """Sends one request and returns the bridge's reply."""
    await bridge.handle(dict(type="request", id=7, op=op, **fields))
    return sent(bridge, "reply")[-1]


def block_window(app, block, sessions=1):
    window = app.add_window(sessions=sessions)
    window.variables[bletherneuk_bridge.BLOCK_VAR] = block
    return window

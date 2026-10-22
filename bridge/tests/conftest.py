"""Puts the fake `iterm2` module in place before the bridge is imported."""

import sys
from pathlib import Path

import fake_iterm2

sys.modules["iterm2"] = fake_iterm2
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

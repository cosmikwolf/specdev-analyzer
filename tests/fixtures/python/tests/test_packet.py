import unittest
import pytest
from midi_py.packet import Packet
from midi_py import stream

def test_roundtrip():
    assert Packet([1])

def helper():
    pass

@pytest.mark.parametrize("x", [1, 2])
def test_param(x):
    pass

async def test_async():
    pass

class TestPacket:
    def test_eq(self):
        pass
    def not_a_test(self):
        pass
    @pytest.mark.slow
    def test_decorated(self):
        pass
    class TestNested:
        def test_inner(self):
            pass

class PacketCase(unittest.TestCase):
    def test_unit(self):
        pass

class Helper:
    def test_ignored(self):
        pass

@pytest.mark.skip(reason="later")
def test_skipped():
    pass

class TestOther:
    def test_eq(self):
        pass

class Plain:
    @pytest.mark.skip
    def test_not_collected(self):
        pass

class TestSkips:
    @unittest.skip("later")
    def test_member_skipped(self):
        pass

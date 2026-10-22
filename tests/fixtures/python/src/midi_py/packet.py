from __future__ import annotations
import enum
from typing import TYPE_CHECKING, TypeAlias
from dataclasses import dataclass
from .util import clamp
from ..other import thing

if TYPE_CHECKING:
    from .stream import Stream

try:
    import orjson
except ImportError:
    orjson = None

MAX_WORDS = 4
_CACHE_SIZE = 16
default_group = 0
Words: TypeAlias = list[int]
type Group = int

def make(words: Words) -> "Packet":
    def inner():
        pass
    return Packet(words)

async def fetch(stream: Stream) -> Packet:
    ...

def _private_helper():
    pass

@dataclass(frozen=True)
class Packet:
    """A UMP packet."""
    MT: int = 0
    words: list[int]

    def __init__(self, words):
        self.words = words

    def __eq__(self, other):
        return True

    @property
    def message_type(self) -> int:
        return self.words[0] >> 28

    @staticmethod
    def parse(data: bytes) -> "Packet":
        return Packet([])

    async def send(self):
        pass

    def _check(self):
        pass

    class Inner:
        def deep(self):
            pass

class _Hidden:
    def visible_member(self):
        pass

class Kind(enum.Enum):
    NOTE = 1

if True:
    def conditional():
        pass

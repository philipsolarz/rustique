import enum
class Color(enum.Enum):
    RED = 1
    GREEN = 2
class K:
    def __hash__(self): return 7
    def __eq__(self, o): return False
class Bad(enum.Enum):
    A = 1
Bad._value2member_map_[K()] = Bad.A

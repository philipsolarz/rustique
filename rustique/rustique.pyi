from builtins import int, float, complex, str, bool, dict, list, tuple, set
from _collections_abc import KeysView as _KeysView, ItemsView as _ItemsView, ValuesView as _ValuesView, MappingView as _MappingView

class Int(int):
    pass

class Float(float):
    pass

class Complex(complex):
    pass

class Str(str):
    pass

class Bool(bool):
    pass

class MappingView(_MappingView):
    pass

class KeysView(_KeysView):
    pass

class ItemsView(_ItemsView):
    pass

class ValuesView(_ValuesView):
    pass

class Dict(dict):
    pass

class List(list):
    pass

class Tuple(tuple):
    pass

class Set(set):
    pass

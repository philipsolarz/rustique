from typing import TypeVar
from rustique import i8, i16, i32, i64, i128, u8, u16, u32, u64, u128, f32, f64, _Vector # , _Vector_i8, _Vector_i16, _Vector_i32, _Vector_i64, _Vector_i128, _Vector_u8, _Vector_u16, _Vector_u32, _Vector_u64, _Vector_u128, _Vector_f32, _Vector_f64
T = TypeVar("T")

class MetaVector(type):
    _type_map = {
        i8: _Vector # _Vector_i8,
        # i16: _Vector_i16,
        # i32: _Vector_i32,
        # i64: _Vector_i64,
        # i128: _Vector_i128,
        # u8: _Vector_u8,
        # u16: _Vector_u16,
        # u32: _Vector_u32,
        # u64: _Vector_u64,
        # u128: _Vector_u128,
        # f32: _Vector_f32,
        # f64: _Vector_f64,
    }

    def __class_getitem__(cls, item):
        print("DEBUG: item =", item)
        print("DEBUG: type_map =", cls._type_map)

        if item not in cls._type_map:
            raise TypeError(f"Unsupported type {item}. Available: {list(cls._type_map.keys())}")
        
        rust_type = cls._type_map[item]
        print("DEBUG: rust_type =", rust_type)
        
        # Ensure name is a string:
        name = getattr(item, '__name__', str(item))
        print("DEBUG: class name =", name)

        return type(
            f"Vector[{name}]",
            (cls,),
            {"_type": item, "_rust_type": rust_type}
        )

class Vector(metaclass=MetaVector):
    _type = None
    _rust_type = None

    def __init__(self, *args):
        if self._type is None:
            raise TypeError("Type argument is required but not provided.")

        # Create empty Rust vector
        self._rust_vec = self._rust_type()

        # Append values
        for arg in args:
            if not isinstance(arg, self._type):
                raise TypeError(f"Expected {self._type}, got {type(arg)}")
            self._rust_vec.append(arg)

    @property
    def values(self):
        return self._rust_vec.to_list()

    def __repr__(self):
        return repr(self._rust_vec)

class Vector[T](_Vector):

    def __init__(self, *args, **kwargs):
        if not hasattr(self, "_type"):
            raise TypeError("Type argument is required but not provided.")
        super().__init__(*args, **kwargs)

    def _validate_all_values(self, values):
        for value in values:
            if not isinstance(value, self._type):
                raise TypeError(f"Value {value} is not of type {self._type}.")

    @classmethod
    def __class_getitem__(cls, item):
        return 

class Vector[T]:

    def __init__(self, *values: T) -> None:
        if not hasattr(self, "_type"):
            raise TypeError("Type argument is required but not provided.")

        # Ensure all values match the provided type argument
        for value in values:
            if not isinstance(value, self._type):
                raise TypeError(f"Value {value} is not of type {self._type}.")
        
        self.values = list(values)
        print(f"Initialized Vector with values: {self.values} of type {self._type}")

    @classmethod
    def __class_getitem__(cls, item):
        # Return a new subclass of Vector to avoid shared state
        class SubVector(cls):
            _type = item
        return SubVector

# Test it
try:
    v1 = Vector[i8](1, 2, 3)
    print(v1.values)
except TypeError as e:
    print(e)

try:
    v2 = Vector[i8]([1, 2, 3])
    print(v2.values)
except TypeError as e:
    print(e)

try:
    v3 = Vector[i8](1, 2, "a")
except TypeError as e:
    print(e)

try:
    v4 = Vector(1, 2, 3)
except TypeError as e:
    print(e)

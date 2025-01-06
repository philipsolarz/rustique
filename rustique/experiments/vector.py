from typing import TypeVar
import rustique as rs

T = TypeVar("T")

class Vector[T](rs.Vector):
    def __new__(cls, *values: T) -> rs.Vector:
        if not hasattr(cls, "_type"):
            return super().__new__(cls, *values)
            # raise TypeError("Type argument is required but not provided.")
        return super().__new__(cls, *values, _type=cls._type)
 
    @classmethod
    def __class_getitem__(cls, item):
        class TypedVector[T](cls):
            _type = item
        return TypedVector

try:
    v = Vector[int](1, 2 , 3)
    print(v)
    v.append(4)
    print(v)
    print(v[0])
    v[0] = 5
    print(v[0])
except TypeError as e:
    print(e)

# try:
#     v1 = Vector(1, 2, 3, "a")
#     print(v1)
# except TypeError as e:
#     print(e)

# try:
#     v2 = Vector[int]([1, 2, 3])
#     print(v2)
# except TypeError as e:
#     print(e)

# try:
#     v3 = Vector[int](1, 2, "a")
# except TypeError as e:
#     print(e)

# try:
#     v4 = Vector(1, 2, 3)
# except TypeError as e:
#     print(e)

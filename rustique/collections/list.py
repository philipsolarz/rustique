from typing import TypeVar
import rustique as rs

T = TypeVar("T")

class List[T](rs.List):
    def __new__(cls, *values: T) -> rs.List:
        if not hasattr(cls, "_type"):
            return super().__new__(cls, *values)
        return super().__new__(cls, *values, _type=cls._type)
 
    @classmethod
    def __class_getitem__(cls, item):
        class TypedList[T](cls):
            _type = item
        return TypedList

import pytest
from rustique import List
import rustique as rs

# class List(rs.List):
#     def __new__(cls, *args, **kwargs): 
#         if kwargs:
#             raise TypeError("List does not accept keyword arguments")
#         if len(args) > 1:
#             raise TypeError(f"List expected at most 1 argument, got {len(args)}")
#         iterable = args[0] if args else ()
#         return super().__new__(cls, *iterable)

        # ls  = list(args[0]) if args else []
        # return super().__new__(cls, ls)


def test_constructor_empty():
    """Test List constructor with no arguments."""
    data = []
    lst = List(data)
    assert len(lst) == len(data)
    assert lst == data

def test_constructor_with_list():
    """Test List constructor with built-in list."""
    data = [1, 2, 3]
    lst = List(data)
    assert len(lst) == len(data)
    assert lst == data
    assert isinstance(lst, List)

    data_empty = []
    lst_empty = List(data_empty)
    assert len(lst_empty) == len(data_empty)
    assert lst_empty == data_empty

    data_nested = [[1, 2], [3, 4]]
    lst_nested = List(data_nested)
    assert len(lst_nested) == len(data_nested)
    assert lst_nested == data_nested

    data_mixed = ['a', 42, None, 3.14]
    lst_mixed = List(data_mixed)
    assert len(lst_mixed) == len(data_mixed)
    assert lst_mixed == data_mixed

    data_large = list(range(1000))
    lst_large = List(data_large)
    assert len(lst_large) == len(data_large)
    assert lst_large[:5] == data_large[:5]

    data_iter = iter([1, 2, 3])
    lst_iter = List(data_iter)
    assert lst_iter == [1, 2, 3]

    data_tuple = (1, 2, 3)
    lst_tuple = List(data_tuple)
    assert len(lst_tuple) == len(data_tuple)
    assert lst_tuple == list(data_tuple)

    data_bytes = b'abc'
    lst_bytes = List(data_bytes)
    assert lst_bytes == list(data_bytes)

    data_set = {1, 2, 3}
    lst_set = List(data_set)
    assert sorted(lst_set) == sorted(list(data_set))

def test_constructor_iterator_exhaustion():
    """Test List constructor consumes iterator fully."""
    data = iter([1, 2, 3])
    lst = List(data)
    assert lst == [1, 2, 3]
    assert list(data) == []  # Ensure the iterator is exhausted

def test_len():
    """Test __len__ method of List."""
    data_empty = []
    lst_empty = List(data_empty)
    assert len(lst_empty) == len(data_empty)

    data_single = [1]
    lst_single = List(data_single)
    assert len(lst_single) == len(data_single)

    data_falsy = [None, '', [], {}]
    lst_falsy = List(data_falsy)
    assert len(lst_falsy) == len(data_falsy)

    data_large = list(range(1000))
    lst_large = List(data_large)
    assert len(lst_large) == len(data_large)

    data_nested = [1, 2, [3, 4], 5]
    lst_nested = List(data_nested)
    assert len(lst_nested) == len(data_nested)

def test_getitem_index():
    """Test __getitem__ with integer indices."""
    data = [10, 20, 30, 40, 50]
    lst = List(data)

    # Test positive indices
    assert lst[0] == data[0]
    assert lst[1] == data[1]
    assert lst[4] == data[4]

    # Test negative indices
    assert lst[-1] == data[-1]
    assert lst[-2] == data[-2]
    assert lst[-5] == data[-5]

    # Test out-of-bounds cases
    with pytest.raises(IndexError):
        _ = lst[len(data)]

    with pytest.raises(IndexError):
        _ = lst[-(len(data) + 1)]

    # Test single-element list
    single_data = [100]
    lst_single = List(single_data)
    assert lst_single[0] == single_data[0]
    assert lst_single[-1] == single_data[-1]

    # Test empty list
    empty_data = []
    lst_empty = List(empty_data)
    with pytest.raises(IndexError):
        _ = lst_empty[0]

    # Test nested lists
    nested_data = [1, [2, 3], [4, [5, 6]]]
    lst_nested = List(nested_data)
    assert lst_nested[1] == nested_data[1]
    assert lst_nested[2] == nested_data[2]
    assert lst_nested[2][1] == nested_data[2][1]
    assert lst_nested[2][1][1] == nested_data[2][1][1]


def test_getitem_slice():
    """Test __getitem__ with slicing."""
    data = [10, 20, 30, 40, 50, 60, 70, 80]
    lst = List(data)

    # Basic slicing
    assert lst[:] == data[:]
    assert lst[2:5] == data[2:5]
    assert lst[:4] == data[:4]
    assert lst[3:] == data[3:]
    assert lst[1:6:2] == data[1:6:2]
    assert lst[::-1] == data[::-1]  # Reverse the list

    # Step-based slicing
    assert lst[::2] == data[::2]
    assert lst[1::2] == data[1::2]
    assert lst[-6:-2] == data[-6:-2]
    assert lst[-1:-6:-2] == data[-1:-6:-2]

    # Out-of-bounds slicing (should not raise an error)
    assert lst[:100] == data[:100]
    assert lst[-100:] == data[-100:]
    assert lst[100:200] == data[100:200]

    # Empty list slicing
    empty_data = []
    lst_empty = List(empty_data)
    assert lst_empty[:] == empty_data[:]
    assert lst_empty[:5] == empty_data[:5]
    assert lst_empty[-5:] == empty_data[-5:]

    # Single-element list slicing
    single_data = [100]
    lst_single = List(single_data)
    assert lst_single[:] == single_data[:]
    assert lst_single[:1] == single_data[:1]
    assert lst_single[::-1] == single_data[::-1]

    # Nested list slicing
    nested_data = [[1, 2], [3, 4], [5, 6]]
    lst_nested = List(nested_data)
    assert lst_nested[1:] == nested_data[1:]
    assert lst_nested[:2] == nested_data[:2]
    assert lst_nested[::2] == nested_data[::2]

    # Ensure slicing does not modify original list
    assert lst[:] == data

def test_setitem_index():
    """Test __setitem__ with integer indices."""
    data = [10, 20, 30, 40, 50]
    lst = List(data)

    # Modify individual elements
    lst[0] = 100
    data[0] = 100
    assert lst == data

    lst[2] = 300
    data[2] = 300
    assert lst == data

    lst[-1] = 500
    data[-1] = 500
    assert lst == data

    lst[-3] = 250
    data[-3] = 250
    assert lst == data

    # Test out-of-bounds cases
    with pytest.raises(IndexError):
        lst[5] = 600  # Beyond upper limit

    with pytest.raises(IndexError):
        lst[-6] = 700  # Beyond lower limit

    # Modify all elements via a loop
    for i in range(len(data)):
        lst[i] = i * 10
        data[i] = i * 10
    assert lst == data

    # Single-element list modification
    single_data = [100]
    lst_single = List(single_data)
    lst_single[0] = 200
    single_data[0] = 200
    assert lst_single == single_data

    # Ensure empty list raises an error when setting an index
    empty_data = []
    lst_empty = List(empty_data)
    with pytest.raises(IndexError):
        lst_empty[0] = 1  # Should raise an IndexError

    # Nested list modification
    nested_data = [[1, 2], [3, 4], [5, 6]]
    lst_nested = List(nested_data)
    lst_nested[1] = [7, 8]
    nested_data[1] = [7, 8]
    assert lst_nested == nested_data

    # Modify an inner list element
    lst_nested[2][1] = 9
    nested_data[2][1] = 9
    assert lst_nested == nested_data


def test_setitem_slice():
    """Test __setitem__ with slice assignment."""
    data = [10, 20, 30, 40, 50, 60, 70, 80]
    lst = List(data)

    # Basic slice assignment
    lst[2:5] = [300, 400, 500]
    data[2:5] = [300, 400, 500]
    assert lst == data

    lst[:3] = [100, 200, 300]
    data[:3] = [100, 200, 300]
    assert lst == data

    lst[4:] = [600, 700, 800]
    data[4:] = [600, 700, 800]
    assert lst == data

    # Step-based slice assignment
    lst[::2] = [1, 2, 3, 4]
    data[::2] = [1, 2, 3, 4]
    assert lst == data

    data = [10, 20, 30, 40, 50, 60, 70, 80]
    lst = List(data)

    lst[1::2] = [9, 8, 7, 6]
    data[1::2] = [9, 8, 7, 6]
    assert lst == data

    # Negative index slicing assignment
    lst[-3:] = [900, 1000, 1100]
    data[-3:] = [900, 1000, 1100]
    assert lst == data

    lst[:-5] = [400, 500, 600]
    data[:-5] = [400, 500, 600]
    assert lst == data

    # Different length assignment
    lst[1:3] = [55]  # Assign fewer elements
    data[1:3] = [55]
    assert lst == data

    lst[1:2] = [11, 22, 33]  # Assign more elements
    data[1:2] = [11, 22, 33]
    assert lst == data

    # Edge case: assign entire list
    lst[:] = [1, 2, 3, 4, 5]
    data[:] = [1, 2, 3, 4, 5]
    assert lst == data

    # Out-of-bounds slice assignment should behave like Python lists
    lst[100:] = [999]
    data[100:] = [999]
    assert lst == data

    lst[-100:] = [777, 888]
    data[-100:] = [777, 888]
    assert lst == data

    # Empty list assignment
    empty_data = []
    lst_empty = List(empty_data)
    lst_empty[:] = [1, 2, 3]
    empty_data[:] = [1, 2, 3]
    assert lst_empty == empty_data

    # Assign an empty slice to remove elements
    lst[2:4] = []
    data[2:4] = []
    assert lst == data

    # Assign with incompatible lengths and expect a ValueError
    with pytest.raises(ValueError):
        lst[1:4:2] = [100, 200, 300]  # Mismatched step length

    # Nested list slice assignment
    nested_data = [[1, 2], [3, 4], [5, 6]]
    lst_nested = List(nested_data)
    lst_nested[:2] = [[7, 8], [9, 10]]
    nested_data[:2] = [[7, 8], [9, 10]]
    assert lst_nested == nested_data

    # Assigning to an empty slice should insert elements
    lst[1:1] = [12, 13]
    data[1:1] = [12, 13]
    assert lst == data


def test_delitem_index():
    """Test __delitem__ with integer indices."""
    data = [10, 20, 30, 40, 50]
    lst = List(data)

    # Delete first element
    del lst[0]
    del data[0]
    assert lst == data

    # Delete middle element
    del lst[2]
    del data[2]
    assert lst == data

    # Delete last element
    del lst[-1]
    del data[-1]
    assert lst == data

    # Delete using negative indices
    del lst[-1]
    del data[-1]
    assert lst == data

    # Test out-of-bounds deletion
    with pytest.raises(IndexError):
        del lst[10]  # Too large

    with pytest.raises(IndexError):
        del lst[-10]  # Too negative

    # Test deletion until empty
    data = [10]
    lst = List(data)
    del lst[0]
    del data[0]
    assert lst == data
    assert len(lst) == 0

    # Ensure deleting from an empty list raises an error
    with pytest.raises(IndexError):
        del lst[0]

    # Single-element list deletion
    single_data = [100]
    lst_single = List(single_data)
    del lst_single[0]
    del single_data[0]
    assert lst_single == single_data

    # Nested list deletion
    nested_data = [[1, 2], [3, 4], [5, 6]]
    lst_nested = List(nested_data)
    del lst_nested[1]
    del nested_data[1]
    assert lst_nested == nested_data

    # Test deleting deeply nested elements
    del lst_nested[1][0]
    del nested_data[1][0]
    assert lst_nested == nested_data


def test_delitem_slice():
    """Test __delitem__ with slicing."""
    data = [10, 20, 30, 40, 50, 60, 70, 80]
    lst = List(data)

    # Delete a slice from the middle
    del lst[2:5]
    del data[2:5]
    assert lst == data

    # Delete from the beginning
    del lst[:2]
    del data[:2]
    assert lst == data

    # Delete from the end
    del lst[-2:]
    del data[-2:]
    assert lst == data

    # Delete with step
    del lst[::2]
    del data[::2]
    assert lst == data

    # Delete a single element using slicing
    del lst[1:2]
    del data[1:2]
    assert lst == data

    # Empty slice deletion (should not change the list)
    del lst[:0]
    del data[:0]
    assert lst == data

    # Delete entire list
    del lst[:]
    del data[:]
    assert lst == data
    assert len(lst) == 0

    # Deleting from an empty list should not fail
    empty_data = []
    lst_empty = List(empty_data)
    del lst_empty[:]
    del empty_data[:]
    assert lst_empty == empty_data

    # Negative step deletion
    data = [10, 20, 30, 40, 50, 60]
    lst = List(data)
    del lst[::-2]
    del data[::-2]
    assert lst == data

    # Deleting every second element
    data = [1, 2, 3, 4, 5, 6, 7, 8]
    lst = List(data)
    del lst[1::2]
    del data[1::2]
    assert lst == data

    # Nested list deletion
    nested_data = [[1, 2], [3, 4], [5, 6]]
    lst_nested = List(nested_data)
    del lst_nested[1:]
    del nested_data[1:]
    assert lst_nested == nested_data


def test_append():
    """Test list.append(x)."""
    data = [10, 20, 30]
    lst = List(data)

    # Append a single element
    lst.append(40)
    data.append(40)
    assert lst == data

    # Append a different data type
    lst.append("hello")
    data.append("hello")
    assert lst == data

    lst.append(None)
    data.append(None)
    assert lst == data

    # Append a nested list
    lst.append([50, 60])
    data.append([50, 60])
    assert lst == data

    # Append an object of different type
    lst.append({'key': 'value'})
    data.append({'key': 'value'})
    assert lst == data

    # Append to an initially empty list
    empty_data = []
    lst_empty = List(empty_data)
    lst_empty.append(100)
    empty_data.append(100)
    assert lst_empty == empty_data

    lst_empty.append([200, 300])
    empty_data.append([200, 300])
    assert lst_empty == empty_data

    # Check list length after appending multiple items
    lst.append(70)
    lst.append(80)
    lst.append(90)
    assert len(lst) == len(data) + 3

    # Append duplicate values
    data = [10, 20, 30, 40]
    lst = List(data)
    lst.append(40)
    data.append(40)
    assert lst == data

    # Verify list is updated correctly
    assert lst[-1] == data[-1]
    assert lst[-2] == data[-2]


def test_extend():
    """Test list.extend(iterable)."""
    data = [10, 20, 30]
    lst = List(data)

    # Extend with another list
    lst.extend([40, 50, 60])
    data.extend([40, 50, 60])
    assert lst == data

    # Extend with a tuple
    lst.extend((70, 80))
    data.extend((70, 80))
    assert lst == data

    # Extend with a set (order may vary)
    lst.extend({90, 100})
    data.extend({90, 100})
    assert sorted(lst) == sorted(data)

    # Extend with a string (should treat as individual characters)
    lst.extend("abc")
    data.extend("abc")
    assert lst == data

    # Extend with a generator
    def num_gen():
        yield 110
        yield 120

    lst.extend(num_gen())
    data.extend(num_gen())
    assert lst == data

    # Extend with an empty iterable
    lst.extend([])
    data.extend([])
    assert lst == data

    # Extend an empty list
    empty_data = []
    lst_empty = List(empty_data)
    lst_empty.extend([1, 2, 3])
    empty_data.extend([1, 2, 3])
    assert lst_empty == empty_data

    # Extend with mixed data types
    lst.extend([None, 3.14, {'key': 'value'}])
    data.extend([None, 3.14, {'key': 'value'}])
    assert lst == data

    # Check final list length
    assert len(lst) == len(data)

    # Ensure extend does not modify the iterable passed
    original = [1, 2, 3]
    copy = original[:]
    lst.extend(original)
    assert original == copy  # Ensure original is unmodified


    # Extend with range
    data = [10, 20, 30]
    lst = List(data)
    lst.extend(range(5))
    data.extend(range(5))
    assert lst == data


def test_insert():
    """Test list.insert(i, x)."""
    data = [10, 20, 30, 40]
    lst = List(data)

    # Insert at the beginning
    lst.insert(0, 5)
    data.insert(0, 5)
    assert lst == data

    # Insert in the middle
    lst.insert(2, 15)
    data.insert(2, 15)
    assert lst == data

    # Insert at the end (equivalent to append)
    lst.insert(len(lst), 50)
    data.insert(len(data), 50)
    assert lst == data

    # Insert using negative index (equivalent to insert at len + index)
    lst.insert(-1, 45)
    data.insert(-1, 45)
    assert lst == data

    # Insert beyond the range (should be equivalent to append)
    lst.insert(100, 60)
    data.insert(100, 60)
    assert lst == data

    # Insert before the start (should be equivalent to index 0)
    lst.insert(-100, 1)
    data.insert(-100, 1)
    assert lst == data

    # Insert into an empty list
    empty_data = []
    lst_empty = List(empty_data)
    lst_empty.insert(0, 100)
    empty_data.insert(0, 100)
    assert lst_empty == empty_data

    # Insert different data types
    lst.insert(3, "hello")
    data.insert(3, "hello")
    assert lst == data

    lst.insert(5, None)
    data.insert(5, None)
    assert lst == data

    lst.insert(7, {'key': 'value'})
    data.insert(7, {'key': 'value'})
    assert lst == data

    # Insert a list
    lst.insert(2, [1, 2, 3])
    data.insert(2, [1, 2, 3])
    assert lst == data

    # Insert with duplicate values
    lst.insert(1, 20)
    data.insert(1, 20)
    assert lst == data

    # Check length after multiple insertions
    assert len(lst) == len(data)


def test_remove():
    """Test list.remove(value)."""
    data = [10, 20, 30, 40, 50, 20]
    lst = List(data)

    # Remove an existing value
    lst.remove(20)
    data.remove(20)
    assert lst == data

    # Remove another occurrence of the same value
    lst.remove(20)
    data.remove(20)
    assert lst == data

    # Remove an element in the middle
    lst.remove(40)
    data.remove(40)
    assert lst == data

    # Remove an element at the beginning
    lst.remove(10)
    data.remove(10)
    assert lst == data

    # Remove an element at the end
    lst.remove(50)
    data.remove(50)
    assert lst == data

    # Attempt to remove a non-existing value (should raise ValueError)
    with pytest.raises(ValueError):
        lst.remove(100)

    # Remove from a list containing multiple data types
    mixed_data = [1, "hello", 3.14, None, [1, 2], "hello"]
    lst_mixed = List(mixed_data)
    lst_mixed.remove("hello")
    mixed_data.remove("hello")
    assert lst_mixed == mixed_data

    # Removing a nested list
    lst_mixed.remove([1, 2])
    mixed_data.remove([1, 2])
    assert lst_mixed == mixed_data

    # Removing None
    lst_mixed.remove(None)
    mixed_data.remove(None)
    assert lst_mixed == mixed_data

    # Attempting to remove from an empty list should raise an error
    empty_data = []
    lst_empty = List(empty_data)
    with pytest.raises(ValueError):
        lst_empty.remove(1)

    # Remove last element, making the list empty
    single_data = [99]
    lst_single = List(single_data)
    lst_single.remove(99)
    single_data.remove(99)
    assert lst_single == single_data
    assert len(lst_single) == 0


def test_pop():
    """Test list.pop([index])."""
    data = [10, 20, 30, 40, 50]
    lst = List(data)

    # Pop last element (default behavior)
    popped = lst.pop()
    expected = data.pop()
    assert popped == expected
    assert lst == data

    # Pop from a specific position
    popped = lst.pop(1)
    expected = data.pop(1)
    assert popped == expected
    assert lst == data

    # Pop first element
    popped = lst.pop(0)
    expected = data.pop(0)
    assert popped == expected
    assert lst == data

    # Pop using negative index (last element)
    popped = lst.pop(-1)
    expected = data.pop(-1)
    assert popped == expected
    assert lst == data

    # Pop the remaining element
    popped = lst.pop(0)
    expected = data.pop(0)
    assert popped == expected
    assert lst == data
    assert len(lst) == 0  # List should now be empty

    # Pop from an empty list should raise IndexError
    with pytest.raises(IndexError):
        lst.pop()

    # Pop from an empty list with index should raise IndexError
    with pytest.raises(IndexError):
        lst.pop(0)

    # Single-element list pop
    single_data = [100]
    lst_single = List(single_data)
    popped = lst_single.pop()
    expected = single_data.pop()
    assert popped == expected
    assert lst_single == single_data
    assert len(lst_single) == 0

    # Pop with out-of-bounds index
    data = [1, 2, 3]
    lst = List(data)
    with pytest.raises(IndexError):
        lst.pop(10)

    with pytest.raises(IndexError):
        lst.pop(-4)

    # Pop elements until list is empty
    data = [1, 2, 3, 4, 5]
    lst = List(data)
    while lst:
        assert lst.pop() == data.pop()
        assert lst == data


def test_clear():
    """Test list.clear()."""
    data = [10, 20, 30, 40, 50]
    lst = List(data)

    # Clear the list and check if it's empty
    lst.clear()
    data.clear()
    assert lst == data
    assert len(lst) == 0

    # Clear an already empty list
    lst.clear()
    data.clear()
    assert lst == data
    assert len(lst) == 0

    # Clear a list with different data types
    mixed_data = [1, "hello", None, 3.14, [1, 2]]
    lst_mixed = List(mixed_data)
    lst_mixed.clear()
    mixed_data.clear()
    assert lst_mixed == mixed_data
    assert len(lst_mixed) == 0

    # Clear a single-element list
    single_data = [100]
    lst_single = List(single_data)
    lst_single.clear()
    single_data.clear()
    assert lst_single == single_data
    assert len(lst_single) == 0

    # Clear a nested list
    nested_data = [[1, 2], [3, 4], [5, 6]]
    lst_nested = List(nested_data)
    lst_nested.clear()
    nested_data.clear()
    assert lst_nested == nested_data
    assert len(lst_nested) == 0

    # Ensure clear does not raise an exception when called multiple times
    lst.clear()
    assert lst == []
    assert len(lst) == 0


def test_index():
    """Test list.index(value)."""
    data = [10, 20, 30, 40, 50, 20, 60]
    lst = List(data)

    # Test finding elements at various positions
    assert lst.index(10) == data.index(10)  # First element
    assert lst.index(30) == data.index(30)  # Middle element
    assert lst.index(60) == data.index(60)  # Last element

    # Test finding duplicate values (should return first occurrence)
    assert lst.index(20) == data.index(20)

    # Test with different data types
    mixed_data = [1, "hello", 3.14, None, [1, 2], "hello"]
    lst_mixed = List(mixed_data)
    assert lst_mixed.index("hello") == mixed_data.index("hello")
    assert lst_mixed.index(None) == mixed_data.index(None)
    assert lst_mixed.index([1, 2]) == mixed_data.index([1, 2])

    # Test index with start and stop parameters
    assert lst.index(20, 2) == data.index(20, 2)  # Start from index 2
    assert lst.index(20, 0, 5) == data.index(20, 0, 5)  # Limit range

    # Test out-of-bounds value (should raise ValueError)
    with pytest.raises(ValueError):
        lst.index(100)

    with pytest.raises(ValueError):
        lst.index("not in list")

    # Test searching within an empty list
    empty_data = []
    lst_empty = List(empty_data)
    with pytest.raises(ValueError):
        lst_empty.index(1)

    # Test index in a list with a single element
    single_data = [99]
    lst_single = List(single_data)
    assert lst_single.index(99) == single_data.index(99)

    with pytest.raises(ValueError):
        lst_single.index(100)

    # Test index with nested lists
    nested_data = [[1, 2], [3, 4], [5, 6]]
    lst_nested = List(nested_data)
    assert lst_nested.index([3, 4]) == nested_data.index([3, 4])
    assert lst_nested.index([5, 6]) == nested_data.index([5, 6])

    # Test index with start/stop that does not include the value
    with pytest.raises(ValueError):
        lst.index(40, 0, 2)  # 40 is not within range [0:2]


def test_count():
    """Test list.count(value)."""
    data = [10, 20, 30, 20, 40, 50, 20, 60, 20]
    lst = List(data)

    # Count occurrences of an element that appears multiple times
    assert lst.count(20) == data.count(20)

    # Count occurrences of an element that appears once
    assert lst.count(30) == data.count(30)

    # Count occurrences of an element that does not exist in the list
    assert lst.count(100) == data.count(100)

    # Count with different data types
    mixed_data = [1, "hello", 3.14, None, "hello", 3.14, 3.14]
    lst_mixed = List(mixed_data)
    assert lst_mixed.count("hello") == mixed_data.count("hello")
    assert lst_mixed.count(3.14) == mixed_data.count(3.14)
    assert lst_mixed.count(None) == mixed_data.count(None)

    # Count nested lists (must match structure exactly)
    nested_data = [[1, 2], [3, 4], [1, 2], [5, 6]]
    lst_nested = List(nested_data)
    assert lst_nested.count([1, 2]) == nested_data.count([1, 2])
    assert lst_nested.count([5, 6]) == nested_data.count([5, 6])
    assert lst_nested.count([7, 8]) == nested_data.count([7, 8])  # Not present

    # Count in an empty list (should always return 0)
    empty_data = []
    lst_empty = List(empty_data)
    assert lst_empty.count(10) == empty_data.count(10)

    # Count in a single-element list
    single_data = [99]
    lst_single = List(single_data)
    assert lst_single.count(99) == single_data.count(99)
    assert lst_single.count(100) == single_data.count(100)  # Not present

    # Count duplicate values
    data_with_duplicates = [1, 1, 1, 2, 3, 1, 4, 1]
    lst_with_duplicates = List(data_with_duplicates)
    assert lst_with_duplicates.count(1) == data_with_duplicates.count(1)
    assert lst_with_duplicates.count(2) == data_with_duplicates.count(2)
    assert lst_with_duplicates.count(5) == data_with_duplicates.count(5)  # Not present


def test_reverse():
    """Test list.reverse()."""
    data = [10, 20, 30, 40, 50]
    lst = List(data)

    # Reverse the list and compare
    lst.reverse()
    data.reverse()
    assert lst == data

    # Reverse again to restore original order
    lst.reverse()
    data.reverse()
    assert lst == data

    # Reverse a list with a single element
    single_data = [99]
    lst_single = List(single_data)
    lst_single.reverse()
    single_data.reverse()
    assert lst_single == single_data

    # Reverse an empty list
    empty_data = []
    lst_empty = List(empty_data)
    lst_empty.reverse()
    empty_data.reverse()
    assert lst_empty == empty_data

    # Reverse a list with an even number of elements
    even_data = [1, 2, 3, 4]
    lst_even = List(even_data)
    lst_even.reverse()
    even_data.reverse()
    assert lst_even == even_data

    # Reverse a list with an odd number of elements
    odd_data = [1, 2, 3, 4, 5]
    lst_odd = List(odd_data)
    lst_odd.reverse()
    odd_data.reverse()
    assert lst_odd == odd_data

    # Reverse a list with duplicate values
    duplicate_data = [1, 2, 2, 3, 3, 3]
    lst_duplicate = List(duplicate_data)
    lst_duplicate.reverse()
    duplicate_data.reverse()
    assert lst_duplicate == duplicate_data

    # Reverse a list with different data types
    mixed_data = [1, "hello", 3.14, None, [5, 6]]
    lst_mixed = List(mixed_data)
    lst_mixed.reverse()
    mixed_data.reverse()
    assert lst_mixed == mixed_data

    # Reverse a nested list
    nested_data = [[1, 2], [3, 4], [5, 6]]
    lst_nested = List(nested_data)
    lst_nested.reverse()
    nested_data.reverse()
    assert lst_nested == nested_data


def test_copy():
    """Test list.copy()."""
    data = [10, 20, 30, 40, 50]
    lst = List(data)

    # Perform copy operation
    lst_copy = lst.copy()
    data_copy = data.copy()

    # Verify that the copy has the same content
    assert lst_copy == data_copy

    # Ensure that the copy is a new object (not referencing the same memory)
    assert lst_copy is not lst
    assert data_copy is not data

    # Verify modifying the copy does not affect the original
    lst_copy.append(60)
    data_copy.append(60)
    assert lst_copy == data_copy
    assert lst != lst_copy  # Original list should remain unchanged

    # Copy an empty list
    empty_data = []
    lst_empty = List(empty_data)
    lst_empty_copy = lst_empty.copy()
    assert lst_empty_copy == empty_data.copy()
    assert lst_empty_copy is not lst_empty

    # Copy a single-element list
    single_data = [99]
    lst_single = List(single_data)
    lst_single_copy = lst_single.copy()
    assert lst_single_copy == single_data.copy()
    assert lst_single_copy is not lst_single

    # Copy a list with duplicate values
    duplicate_data = [1, 2, 2, 3, 3, 3]
    lst_duplicate = List(duplicate_data)
    lst_duplicate_copy = lst_duplicate.copy()
    assert lst_duplicate_copy == duplicate_data.copy()
    assert lst_duplicate_copy is not lst_duplicate

    # Copy a list with mixed data types
    mixed_data = [1, "hello", 3.14, None, [5, 6]]
    lst_mixed = List(mixed_data)
    lst_mixed_copy = lst_mixed.copy()
    assert lst_mixed_copy == mixed_data.copy()
    assert lst_mixed_copy is not lst_mixed

    # Copying should not create shallow references to nested lists
    lst_mixed_copy[4].append(7)
    assert lst_mixed[4] == mixed_data[4]  # Should be affected (shallow copy)

    # # Deepcopy test (should be done manually if required)
    # import copy
    # deep_copy = copy.deepcopy(lst_mixed)
    # assert deep_copy == lst_mixed
    # assert deep_copy is not lst_mixed
    # deep_copy[4].append(8)
    # assert deep_copy != lst_mixed  # Deep copy should be independent


def test_contains():
    """Test __contains__."""
    data = [10, 20, 30, 40, 50]
    lst = List(data)

    # Test elements that exist in the list
    assert (10 in lst) == (10 in data)
    assert (30 in lst) == (30 in data)
    assert (50 in lst) == (50 in data)

    # Test elements that do not exist in the list
    assert (100 in lst) == (100 in data)
    assert (0 in lst) == (0 in data)

    # Test with different data types
    mixed_data = [1, "hello", 3.14, None, [1, 2]]
    lst_mixed = List(mixed_data)

    assert ("hello" in lst_mixed) == ("hello" in mixed_data)
    assert (3.14 in lst_mixed) == (3.14 in mixed_data)
    assert (None in lst_mixed) == (None in mixed_data)
    assert ([1, 2] in lst_mixed) == ([1, 2] in mixed_data)
    assert ("world" in lst_mixed) == ("world" in mixed_data)

    # Test with an empty list
    empty_data = []
    lst_empty = List(empty_data)
    assert (10 in lst_empty) == (10 in empty_data)
    assert (None in lst_empty) == (None in empty_data)

    # Test with a list containing duplicates
    duplicate_data = [1, 2, 2, 3, 3, 3]
    lst_duplicate = List(duplicate_data)

    assert (2 in lst_duplicate) == (2 in duplicate_data)
    assert (3 in lst_duplicate) == (3 in duplicate_data)
    assert (4 in lst_duplicate) == (4 in duplicate_data)

    # Test with nested lists
    nested_data = [[1, 2], [3, 4], [5, 6]]
    lst_nested = List(nested_data)

    assert ([1, 2] in lst_nested) == ([1, 2] in nested_data)
    assert ([7, 8] in lst_nested) == ([7, 8] in nested_data)

    # Test with objects
    class CustomObj:
        def __init__(self, value):
            self.value = value

        def __eq__(self, other):
            return isinstance(other, CustomObj) and self.value == other.value

    obj1 = CustomObj(1)
    obj2 = CustomObj(2)
    obj_list = List([obj1, obj2])
    assert (obj1 in obj_list) == (obj1 in [obj1, obj2])
    assert (CustomObj(1) in obj_list) == (CustomObj(1) in [obj1, obj2])  # Should return True if __eq__ works correctly
    assert (CustomObj(3) in obj_list) == (CustomObj(3) in [obj1, obj2])  # Should return False



def test_eq():
    """Test __eq__."""

    # Test equality with identical lists
    data1 = [10, 20, 30, 40, 50]
    data2 = [10, 20, 30, 40, 50]
    lst1 = List(data1)
    lst2 = List(data2)

    assert (lst1 == lst2) == (data1 == data2)
    assert (lst1 == data1) == (data1 == data1)
    assert (lst2 == data2) == (data2 == data2)

    # Test equality with lists having different values
    data3 = [10, 20, 30, 40, 60]
    lst3 = List(data3)
    assert (lst1 == lst3) == (data1 == data3)

    # Test equality with lists of different lengths
    short_data = [10, 20, 30]
    lst_short = List(short_data)
    assert (lst1 == lst_short) == (data1 == short_data)

    long_data = [10, 20, 30, 40, 50, 60]
    lst_long = List(long_data)
    assert (lst1 == lst_long) == (data1 == long_data)

    # Test equality with an empty list
    empty_data = []
    lst_empty = List(empty_data)
    assert (lst1 == lst_empty) == (data1 == empty_data)
    assert (lst_empty == empty_data) == (empty_data == empty_data)

    # Test equality with different data types
    assert (lst1 == "not a list") == (data1 == "not a list")
    assert (lst1 == None) == (data1 == None)
    assert (lst1 == 123) == (data1 == 123)

    # Test equality with nested lists
    nested_data1 = [[1, 2], [3, 4], [5, 6]]
    nested_data2 = [[1, 2], [3, 4], [5, 6]]
    lst_nested1 = List(nested_data1)
    lst_nested2 = List(nested_data2)
    assert (lst_nested1 == lst_nested2) == (nested_data1 == nested_data2)

    nested_data_diff = [[1, 2], [3, 4], [7, 8]]
    lst_nested_diff = List(nested_data_diff)
    assert (lst_nested1 == lst_nested_diff) == (nested_data1 == nested_data_diff)

    # Test equality with lists containing mutable elements
    mutable_data1 = [[1, 2], [3, 4]]
    mutable_data2 = [[1, 2], [3, 4]]
    lst_mutable1 = List(mutable_data1)
    lst_mutable2 = List(mutable_data2)

    lst_mutable1[1][1] = 99  # Modify an element inside
    assert (lst_mutable1 == lst_mutable2) == (mutable_data1 == mutable_data2)

    # Test equality with lists containing duplicate values
    duplicate_data1 = [1, 2, 2, 3, 3, 3]
    duplicate_data2 = [1, 2, 2, 3, 3, 3]
    lst_duplicate1 = List(duplicate_data1)
    lst_duplicate2 = List(duplicate_data2)
    assert (lst_duplicate1 == lst_duplicate2) == (duplicate_data1 == duplicate_data2)

    # Test equality with mixed data types
    mixed_data1 = [1, "hello", 3.14, None, [5, 6]]
    mixed_data2 = [1, "hello", 3.14, None, [5, 6]]
    lst_mixed1 = List(mixed_data1)
    lst_mixed2 = List(mixed_data2)
    assert (lst_mixed1 == lst_mixed2) == (mixed_data1 == mixed_data2)

    # Test equality with different objects
    class CustomObj:
        def __init__(self, value):
            self.value = value

        def __eq__(self, other):
            return isinstance(other, CustomObj) and self.value == other.value

    obj1 = CustomObj(1)
    obj2 = CustomObj(1)
    obj3 = CustomObj(2)

    obj_list1 = List([obj1, obj2])
    obj_list2 = List([obj1, obj2])
    obj_list3 = List([obj1, obj3])

    assert (obj_list1 == obj_list2) == ([obj1, obj2] == [obj1, obj2])
    assert (obj_list1 == obj_list3) == ([obj1, obj2] == [obj1, obj3])


def test_mul():
    """Test __mul__ and __rmul__."""
    data = [1, 2, 3]
    lst = List(data)

    # Test multiplication by positive integers
    assert lst * 3 == data * 3
    assert 3 * lst == 3 * data
    assert lst * 1 == data * 1  # Multiplying by 1 should return the same list
    assert 1 * lst == 1 * data

    # Test multiplication by zero (should return an empty list)
    assert lst * 0 == data * 0
    assert 0 * lst == 0 * data

    # Test multiplication by negative numbers (should return an empty list)
    assert lst * -1 == data * -1
    assert -1 * lst == -1 * data

    # Ensure original list remains unchanged
    assert lst == data

    # Test multiplication with an empty list
    empty_data = []
    lst_empty = List(empty_data)
    assert lst_empty * 3 == empty_data * 3
    assert 3 * lst_empty == 3 * empty_data
    assert lst_empty * 0 == empty_data * 0

    # Test single-element list multiplication
    single_data = [99]
    lst_single = List(single_data)
    assert lst_single * 4 == single_data * 4
    assert 4 * lst_single == 4 * single_data

    # Test mixed data types multiplication
    mixed_data = [1, "hello", 3.14, None]
    lst_mixed = List(mixed_data)
    assert lst_mixed * 2 == mixed_data * 2
    assert 2 * lst_mixed == 2 * mixed_data

    # Test multiplication resulting in large lists
    large_data = list(range(10))
    lst_large = List(large_data)
    assert lst_large * 5 == large_data * 5
    assert 5 * lst_large == 5 * large_data

    # Test edge case with complex nested lists
    nested_data = [[1, 2], [3, 4]]
    lst_nested = List(nested_data)
    assert lst_nested * 2 == nested_data * 2
    assert 2 * lst_nested == 2 * nested_data


from rustique import List

def test_list():
    # Test creation of an empty list
    lst = List()
    print("Empty list:", lst)  # Expect: []

    # Test creation with initial elements
    lst2 = List([1, 2, 3])
    print("List with elements:", lst2)  # Expect: [1, 2, 3]

    # Test appending elements
    lst2.append(4)
    print("After appending 4:", lst2)  # Expect: [1, 2, 3, 4]

    # Test sum of elements
    print("Sum of elements:", lst2.sum())  # Expect: 10

    # Test length
    print("Length of list:", len(lst2))  # Expect: 4

    # Test indexing (__getitem__)
    print("Element at index 1:", lst2[1])  # Expect: 2

    # Test setting an element (__setitem__)
    lst2[1] = 99
    print("After setting index 1 to 99:", lst2)  # Expect: [1, 99, 3, 4]

    # Test deleting an element (__delitem__)
    del lst2[2]
    print("After deleting index 2:", lst2)  # Expect: [1, 99, 4]

    # Test __contains__
    print("Contains 99:", 99 in lst2)  # Expect: True
    print("Contains 3:", 3 in lst2)    # Expect: False

    # Test __str__ and __repr__
    print("String representation:", str(lst2))  # Expect: [1, 99, 4]
    print("Repr representation:", repr(lst2))  # Expect: List([1, 99, 4])

    # Test addition (__add__)
    lst3 = lst2 + List([5, 6])
    print("After adding [5, 6]:", lst3)  # Expect: [1, 99, 4, 5, 6]

    # Test multiplication (__mul__)
    lst4 = lst2 * 2
    print("After multiplying by 2:", lst4)  # Expect: [1, 99, 4, 1, 99, 4]

if __name__ == "__main__":
    test_list()

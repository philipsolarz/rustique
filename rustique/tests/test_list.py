from rustique import List

def test_list_operations():
    lst = List()
    # assert lst == []
    
    lst2 = List([1, 2, 3])
    assert lst2 == [1, 2, 3]
    
    lst2.append(4)
    assert lst2 == [1, 2, 3, 4]
    
    assert lst2.sum() == 10
    assert len(lst2) == 4
    assert lst2[1] == 2
    
    lst2[1] = 99
    assert lst2 == [1, 99, 3, 4]
    
    del lst2[2]
    assert lst2 == [1, 99, 4]
    
    assert 99 in lst2
    assert 3 not in lst2
    
    assert str(lst2) == "[1, 99, 4]"
    assert repr(lst2) == "List([1, 99, 4])"
    
    lst3 = lst2 + List([5, 6])
    assert lst3 == [1, 99, 4, 5, 6]
    
    lst4 = lst2 * 2
    assert lst4 == [1, 99, 4, 1, 99, 4]
    
    print("All tests passed!")
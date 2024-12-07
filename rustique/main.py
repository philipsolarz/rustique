# from rustique import List
import rustique as rs

def test_num():
    a = rs.int(1)
    assert a == rs.int(1)
    assert a == 1
    # assert a == 1.0
    assert 1 == 1.0
    # assert a < rs.int(2)
    # assert a <= rs.int(2)
    # assert a > rs.int(0)
    # assert a >= rs.int(0)



def test0_eq():
    a = rs.list()
    assert a == rs.list()
    # assert a == []
    # assert a == rs.list([])

    # b = rs.list([1, 2, 3])
    # assert b == rs.list([1, 2, 3])
    # assert b == [1, 2, 3]

    # assert not a == b

def tests():
    test_num()
    # test0_eq()

if __name__ == "__main__":
    tests()
    print("Rustique is working!")

# from rustique import List
import rustique as rs
from rustique import i32
def test_i32():
    
    x: i32 = i32(1)






def test_num():
    a = int(1)
    assert a == 1
    assert a == 1.0
    assert a == rs.int(1)
    assert a == rs.int(1.0)
    assert a == rs.int(1.1)
    assert not a == 1.1
    assert a == rs.int(rs.int(1))

    assert rs.int(1) + rs.int(1) == 2
    assert rs.int(1) + 1 == 2
    # b = rs.int(100**1000)
    # # assert b == 100**1000

    # assert a == 1.0

    # b = rs.int(1.0)
    # assert a == 1.0000000000000001
    # assert a == rs.int(1)
    # assert a == 1
    # assert a == 1.0
    # assert 1 == 1.0
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
    test_i32()
    # test_num()
    # test0_eq()

if __name__ == "__main__":
    tests()
    print("Rustique is working!")

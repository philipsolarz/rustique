from rustique import i8
import timeit

print(f"Python native int initialization time   : {timeit.timeit(lambda: int(42), number=100):.12f} seconds")
# print(f"Rust i8_ initialization time            : {timeit.timeit(lambda: i8_(42), number=100):.12f} seconds")
print(f"Rust i8 initialization time             : {timeit.timeit(lambda: i8(42), number=100):.12f} seconds")

x = i8(42)

assert x == 42
assert x != 43
assert x < 43
assert x <= 43
assert x > 41
assert x >= 41
assert x + 1 == 43
assert x - 1 == 41
assert x * 2 == 84
assert x / 2 == 21
assert x // 2 == 21
assert x % 2 == 0
assert -x == -42
assert +x == 42
assert abs(x) == 42

print("All tests passed!")

y = i8(i8(42))
z = i8(4200000000000000000000.0)
print(i8("123"))
# assert x == y
print(i8(127) + i8(1))
print(i8(127).checked_add(i8(1)))
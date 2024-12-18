import rustique as rs
import timeit
import random

random_int = random.randint(1, 1000)
# Test inputs
small_int = 2**1
large_int = 2**1000000
medium_int = 2**62
# Python native int initialization

# Benchmark Python native int initialization
# Use lambda functions to pass arguments to timeit
python_int_time = timeit.timeit(lambda: int(medium_int), number=10000000)

# Benchmark Rust-backed rug::Integer initialization
# Use lambda functions to pass arguments to timeit
rug_int_time = timeit.timeit(lambda: rs.i64(medium_int), number=10000000)

# Print Results
print(f"Python native int initialization time   : {python_int_time:.12f} seconds")
print(f"Rust i32 initialization time            : {rug_int_time:.12f} seconds")

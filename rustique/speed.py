import rustique as rs
import timeit

# Test inputs
small_int = 42
large_int = 1000000**1000000
medium_int = 1000**1000
# Python native int initialization

# Benchmark Python native int initialization
# Use lambda functions to pass arguments to timeit
python_int_time = timeit.timeit(lambda: int(medium_int), number=1000)

# Benchmark Rust-backed rug::Integer initialization
# Use lambda functions to pass arguments to timeit
rug_int_time = timeit.timeit(lambda: rs.int(medium_int), number=1000)

# Print Results
print(f"Python native int initialization time: {python_int_time:.6f} seconds")
print(f"Rust rug::Integer initialization time: {rug_int_time:.6f} seconds")

import rustique as rs
import timeit

# Test inputs
small_int = 42
large_int = 20**53

# Python native int initialization
def python_int_init():
    x = int(large_int)  # Python int

# Rust rug::Integer initialization
def rust_rug_integer_init():
    x = rs.int(large_int)  # Rust-backed rug::Integer

# Benchmark Python native int initialization
python_int_time = timeit.timeit(python_int_init, number=10000000)

# Benchmark Rust-backed rug::Integer initialization

rug_int_time = timeit.timeit(rust_rug_integer_init, number=10000000)

# Print Results
print(f"Python native int initialization time: {python_int_time:.6f} seconds")
print(f"Rust rug::Integer initialization time: {rug_int_time:.6f} seconds")

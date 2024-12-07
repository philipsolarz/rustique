import sys
sys.set_int_max_str_digits(30810)
# Generate 1028 powers of 2^30
powers_of_2_30 = [str(2**(30 * i)) for i in range(1028)]

# Output to a file or print to console
with open("powers_of_2_30.txt", "w") as f:
    for power in powers_of_2_30:
        f.write(f"{power}\n")

print("Powers of 2^30 generated and saved to powers_of_2_30.txt")

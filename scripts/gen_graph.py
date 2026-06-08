#!/usr/bin/env python3
"""Generate a synthetic social RDF graph (N-Triples) with community structure,
for benchmarking. Deterministic given the seed.

Usage: python3 gen_graph.py PEOPLE KNOWS_PER COMMUNITY_SIZE > out.nt
"""
import random
import sys

people = int(sys.argv[1]) if len(sys.argv) > 1 else 20000
knows_per = int(sys.argv[2]) if len(sys.argv) > 2 else 5
comm_size = int(sys.argv[3]) if len(sys.argv) > 3 else 100
random.seed(42)

B = "http://ex/"
out = sys.stdout.write

n_comm = max(1, people // comm_size)


def comm_of(i):
    return i // comm_size


for i in range(people):
    out(f"<{B}p{i}> <{B}age> \"{18 + (i % 60)}\"^^<http://www.w3.org/2001/XMLSchema#integer> .\n")
    out(f"<{B}p{i}> <{B}name> \"Person {i}\" .\n")
    c = comm_of(i)
    lo, hi = c * comm_size, min(people, (c + 1) * comm_size)
    for _ in range(knows_per):
        # 90% within community, 10% across — gives the pyramid real structure.
        if random.random() < 0.9 and hi - lo > 1:
            j = random.randrange(lo, hi)
        else:
            j = random.randrange(people)
        if j != i:
            out(f"<{B}p{i}> <{B}knows> <{B}p{j}> .\n")

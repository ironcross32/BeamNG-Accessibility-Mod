"""How often can a route turn a car round, and could a car drive what it would use?

For sample approaches on two-way roads, follow the forward-only search until it re-enters
the road it came from in the opposite direction, and measure the loop it used.

usage: python -I turnaround_survey.py <map_survey_dir> <roads_dir>
"""
import collections
import heapq
import json
import math
import pathlib
import random
import sys

tools, roads_dir = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])
source = (tools / "analyse_roads.py").read_text(encoding="utf-8").rsplit("\nmain()", 1)[0]
ns = {}
exec(compile(source, "analyse_roads", "exec"), ns)
build, norm, heading, flat = ns["build"], ns["norm"], ns["norm"] and ns["heading"], ns["flat"]


def fillet(graph, a, b, c):
    """Largest arc that fits the corner a-b-c inside its two links, as a turning radius."""
    pa, pb, pc = graph.pos[a], graph.pos[b], graph.pos[c]
    theta = abs(norm(heading(pb, pc) - heading(pa, pb)))
    if theta < 1:
        return math.inf
    return min(flat(pa, pb), flat(pb, pc)) / math.tan(math.radians(min(theta, 179)) / 2)


def turnaround(graph, a, b, limit=2000.0, min_radius=0.0):
    """Shortest forward-only way from a->b back onto b->a; returns (length, path) or None."""
    heap = [(flat(graph.pos[a], graph.pos[b]), 0, (a, b))]
    best, parent, count = {(a, b): heap[0][0]}, {}, 1
    while heap:
        cost, _, state = heapq.heappop(heap)
        if cost > best[state] or cost > limit:
            continue
        x, y = state
        if state == (b, a):
            path, s = [a], state
            nodes = []
            while s in parent:
                nodes.append(s[1])
                s = parent[s]
            nodes.append(s[1])
            return cost, [a] + nodes[::-1]
        approach = heading(graph.pos[x], graph.pos[y])
        for other, edge in graph.adj[y]:
            if other == x or (edge["oneWay"] and edge["a"] != y):
                continue
            if abs(norm(heading(graph.pos[y], graph.pos[other]) - approach)) > 150:
                continue
            if min_radius and fillet(graph, x, y, other) < min_radius:
                continue
            new = cost + edge["length"]
            if new < best.get((y, other), math.inf):
                best[(y, other)] = new
                parent[(y, other)] = state
                heapq.heappush(heap, (new, count, (y, other)))
                count += 1
    return None


def tightest(graph, path):
    radii = [fillet(graph, path[i], path[i + 1], path[i + 2]) for i in range(len(path) - 2)]
    return min(radii) if radii else math.inf


def main():
    random.seed(11)
    print(f"{'level':22}{'samples':>8}{'none<2km':>9}{'loop<60':>9}{'60-150':>8}{'150-500':>9}{'>500':>6}"
          f"{'min r<5':>9}{'r 5-8':>7}{'r>=8':>6}{'none@r8':>9}")
    for path in sorted(roads_dir.glob("*.json")):
        level = json.loads(path.read_text(encoding="utf-8"))
        graph = build(level["roads"])
        if len(graph.pos) < 100:
            continue
        twoway = [e for e in graph.edges if not e["oneWay"] and e["length"] > 10]
        if not twoway:
            continue
        tally = collections.Counter()
        for _ in range(250):
            e = random.choice(twoway)
            a, b = (e["a"], e["b"]) if random.random() < 0.5 else (e["b"], e["a"])
            tally["samples"] += 1
            found = turnaround(graph, a, b)
            if not found:
                tally["none"] += 1
            else:
                length, nodes = found
                band = "lt60" if length < 60 else "lt150" if length < 150 else "lt500" if length < 500 else "big"
                tally[band] += 1
                r = tightest(graph, nodes)
                tally["r_lt5" if r < 5 else "r_lt8" if r < 8 else "r_ok"] += 1
            if not turnaround(graph, a, b, min_radius=8.0):
                tally["none_r8"] += 1
        print(f"{level['level']:22}{tally['samples']:8}{tally['none']:9}{tally['lt60']:9}{tally['lt150']:8}"
              f"{tally['lt500']:9}{tally['big']:6}{tally['r_lt5']:9}{tally['r_lt8']:7}{tally['r_ok']:6}{tally['none_r8']:9}")


main()

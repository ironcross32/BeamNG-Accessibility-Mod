"""Drive simulated routes over every map and count what each candidate rule would say.

usage: python -I route_rules.py <tools_dir> <roads_dir>
"""
import collections
import heapq
import importlib.util
import json
import math
import pathlib
import random
import sys

tools, roads_dir = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])
source = (tools / "analyse_roads.py").read_text(encoding="utf-8").rsplit("\nmain()", 1)[0]
ns = {}
exec(compile(source, "analyse_roads", "exec"), ns)
build, norm, heading, flat = ns["build"], ns["norm"], ns["heading"], ns["flat"]


def largest_component(graph):
    seen, best = set(), []
    for start in graph.adj:
        if start in seen:
            continue
        stack, comp = [start], []
        seen.add(start)
        while stack:
            n = stack.pop()
            comp.append(n)
            for o, _ in graph.adj[n]:
                if o not in seen:
                    seen.add(o)
                    stack.append(o)
        if len(comp) > len(best):
            best = comp
    return best


def route(graph, start, goal):
    """Edge-state Dijkstra: no reversal, nothing sharper than 150, like routeNeighbours."""
    heap, count = [], 0
    best, parent = {}, {}
    for other, edge in graph.adj[start]:
        if edge["oneWay"] and edge["a"] != start:
            continue
        state = (start, other)
        best[state] = edge["length"]
        heapq.heappush(heap, (edge["length"], count, state))
        count += 1
    while heap:
        cost, _, state = heapq.heappop(heap)
        if cost > best.get(state, math.inf):
            continue
        a, b = state
        if b == goal:
            path = [b, a]
            while state in parent:
                state = parent[state]
                path.append(state[0])
            return path[::-1]
        approach = heading(graph.pos[a], graph.pos[b])
        for other, edge in graph.adj[b]:
            if other == a or (edge["oneWay"] and edge["a"] != b):
                continue
            if abs(norm(heading(graph.pos[b], graph.pos[other]) - approach)) > 150:
                continue
            step = edge["length"] * (3 if edge["type"] == "private" else 1)
            state2 = (b, other)
            if cost + step < best.get(state2, math.inf):
                best[state2] = cost + step
                parent[state2] = state
                heapq.heappush(heap, (cost + step, count, state2))
                count += 1
    return None


def edge_between(graph, a, b):
    for o, e in graph.adj[a]:
        if o == b:
            return e
    return None


def exits(graph, a, b):
    approach = heading(graph.pos[a], graph.pos[b])
    out = []
    for other, edge in graph.adj[b]:
        if other == a or (edge["oneWay"] and edge["a"] != b):
            continue
        bearing = norm(heading(graph.pos[b], graph.pos[other]) - approach)
        if abs(bearing) <= 150:
            out.append((other, edge, bearing))
    return out


def settled(graph, path, i, metres=25.0):
    # Heading of the route once `metres` past node path[i].
    travelled, h = 0.0, None
    for j in range(i, len(path) - 1):
        h = heading(graph.pos[path[j]], graph.pos[path[j + 1]])
        travelled += flat(graph.pos[path[j]], graph.pos[path[j + 1]])
        if travelled >= metres:
            break
    return h


def arm_length(graph, node, first, limit=150.0):
    """How far an exit runs before it branches again or ends: a slip link is short."""
    prev, cur = node, first
    total = flat(graph.pos[prev], graph.pos[cur])
    for _ in range(40):
        if total >= limit:
            break
        onward = [o for o, _ in graph.adj[cur] if o != prev]
        if len(onward) != 1:
            break
        total += flat(graph.pos[cur], graph.pos[onward[0]])
        prev, cur = cur, onward[0]
    return total


def comparable(side, chosen):
    """Side road of the same order as the road being driven, judged only relatively."""
    if side["type"] == "private" and chosen["type"] != "private":
        return False
    return side["drv"] >= 0.7 * chosen["drv"] and side["width"] >= 0.7 * chosen["width"]


def evaluate(graph, path):
    tally = collections.Counter()
    km = sum(flat(graph.pos[path[i]], graph.pos[path[i + 1]]) for i in range(len(path) - 1)) / 1000
    tally["km"] = km
    i = 1
    while i < len(path) - 1:
        node, prev = path[i], path[i - 1]
        ex = exits(graph, prev, node)
        if len(ex) < 2:
            i += 1
            continue
        # Grouped junction: short links that branch.
        last = i
        while last + 1 < len(path) - 1 and flat(graph.pos[path[last]], graph.pos[path[last + 1]]) <= 35 \
                and len(exits(graph, path[last], path[last + 1])) >= 2:
            last += 1
        approach = heading(graph.pos[prev], graph.pos[node])
        h = settled(graph, path, last)
        net = norm(h - approach) if h is not None else 0.0
        chosen = next((x for x in ex if x[0] == path[i + 1]), None)
        if chosen is None:
            i = last + 1
            continue
        first = chosen[2]
        others = [x for x in ex if x is not chosen]
        tally["branch"] += 1
        # R0: what ran on the first drive. R1: what is live now.
        for rule, min_b in (("R0", 0.0), ("R1", 10.0)):
            if abs(net) > 30:
                tally[rule + "_turn"] += 1
            elif abs(first) > min_b and any(abs(o[2]) <= 45 for o in others):
                tally[rule + "_keep"] += 1
        # R2: "the road continues": the route takes the best-aligned exit, nothing else is
        # near it, and it is no sharper than a bend. Otherwise something has to be said.
        best_other = min((abs(o[2]) for o in others), default=999.0)
        continuing = abs(first) <= 45 and best_other - abs(first) >= 20
        tally["continuing" if continuing else "not_continuing"] += 1
        if continuing:
            # The road carries on, but a net turn past 30 on a road with no rival is only a bend.
            if abs(net) > 30:
                tally["R1_turn_was_bend"] += 1
        else:
            if abs(net) > 30:
                tally["R2_turn"] += 1
            else:
                tally["R2_keep"] += 1
        # R3 = R2, but leaving the bigger road is spoken even if geometrically straightest.
        leaves_major = continuing and any(comparable(chosen[1], o[1]) is False for o in others)
        if continuing and leaves_major:
            tally["R3_extra"] += 1
        lesser = any(comparable(chosen[1], o[1]) is False and abs(o[2]) <= 45 for o in others)
        road_goes_on = continuing and not lesser
        if not road_goes_on:
            if abs(net) > 30:
                tally["F_turn"] += 1
            elif (abs(first) > 10 and best_other <= 45) or lesser:
                tally["F_keep"] += 1
        if road_goes_on and abs(first) <= 10 and best_other <= 45:
            tally["F_silent_past_slip"] += 1
        real_cross = [o for o in others if 50 <= abs(o[2]) <= 130 and comparable(o[1], chosen[1])
                      and arm_length(graph, node, o[0]) >= 60]
        any_cross = [o for o in others if 50 <= abs(o[2]) <= 130]
        if continuing and any_cross:
            tally["straight_past_side"] += 1
        if continuing and real_cross:
            tally["C2_extra"] += 1
        i = last + 1
    return tally


def main():
    random.seed(7)
    print(f"{'level':22}{'routes':>7}{'km':>7}{'branch/km':>10}{'R0 keep':>9}{'R1 keep':>9}{'R1 turn':>9}"
          f"{'R2 keep':>9}{'R2 turn':>9}{'bends':>7}{'F keep':>8}{'F turn':>8}{'past side':>10}")
    total = collections.Counter()
    for path in sorted(roads_dir.glob("*.json")):
        level = json.loads(path.read_text(encoding="utf-8"))
        graph = build(level["roads"])
        if len(graph.pos) < 100:
            continue
        comp = largest_component(graph)
        tally = collections.Counter()
        routes = 0
        for _ in range(300):
            if routes >= 40:
                break
            a, b = random.choice(comp), random.choice(comp)
            if flat(graph.pos[a], graph.pos[b]) < 500:
                continue
            p = route(graph, a, b)
            if p and len(p) > 3:
                tally += evaluate(graph, p)
                routes += 1
        km = max(tally["km"], 0.01)
        total += tally
        print(f"{level['level']:22}{routes:7}{km:7.0f}{tally['branch']/km:10.1f}"
              f"{tally['R0_keep']/km:9.2f}{tally['R1_keep']/km:9.2f}{tally['R1_turn']/km:9.2f}"
              f"{tally['R2_keep']/km:9.2f}{tally['R2_turn']/km:9.2f}{tally['R1_turn_was_bend']/km:7.2f}"
              f"{tally['F_keep']/km:8.2f}{tally['F_turn']/km:8.2f}{tally['straight_past_side']/km:10.2f}")
    print("\nall rates are per km of route")


main()

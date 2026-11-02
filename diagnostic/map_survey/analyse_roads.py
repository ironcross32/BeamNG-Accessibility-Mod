"""Rebuild an approximate road graph per level and measure junction patterns.

usage: python -I analyse_roads.py <roads_dir>
Graph: control points as nodes; a road end on another road's centreline makes a T, crossing
centrelines make an X (map.lua's resolveTJunctions / resolveXJunctions, approximately).
"""
import collections
import json
import math
import pathlib
import statistics
import sys


def norm(a):
    while a > 180:
        a -= 360
    while a < -180:
        a += 360
    return a


def heading(a, b):
    return math.degrees(math.atan2(b[1] - a[1], b[0] - a[0]))


def flat(a, b):
    return math.hypot(a[0] - b[0], a[1] - b[1])


class Graph:
    def __init__(self):
        self.pos = {}
        self.edges = []  # dict with a, b, length, attrs
        self.adj = collections.defaultdict(list)

    def add_edge(self, a, b, attrs):
        if a == b:
            return
        length = flat(self.pos[a], self.pos[b])
        if length < 0.05:
            return
        edge = dict(attrs, a=a, b=b, length=length)
        self.edges.append(edge)
        self.adj[a].append((b, edge))
        self.adj[b].append((a, edge))


def build(roads):
    # Split each road's polyline wherever another road's end lands on it or a centreline crosses it.
    polys = []
    for road in roads:
        if (road.get("drivability") or 0) <= 0 or road.get("hiddenInNavi"):
            continue
        pts = [tuple(p[:4]) for p in road["nodes"] if len(p) >= 4]
        if len(pts) < 2:
            continue
        kind = "private" if road.get("gatedRoad") else (road.get("type") or "")
        one_way = bool(road.get("oneWay"))
        if road.get("autoLanes") is False:
            left, right = road.get("lanesLeft") or 0, road.get("lanesRight") or 0
            one_way = (left == 0) != (right == 0)
        polys.append({"pts": pts, "drv": road["drivability"], "type": kind, "oneWay": one_way,
                      "speed": road.get("speedLimit"), "name": road.get("name") or "",
                      "merge": road.get("autoJunction", True) is not False})
    cell = 60.0
    grid = collections.defaultdict(list)
    for pi, poly in enumerate(polys):
        for si in range(len(poly["pts"]) - 1):
            a, b = poly["pts"][si], poly["pts"][si + 1]
            x0, x1 = sorted((a[0], b[0]))
            y0, y1 = sorted((a[1], b[1]))
            for gx in range(int(x0 // cell), int(x1 // cell) + 1):
                for gy in range(int(y0 // cell), int(y1 // cell) + 1):
                    grid[(gx, gy)].append((pi, si))

    graph = Graph()
    node_at = {}

    def node_for(p):
        key = (round(p[0], 1), round(p[1], 1), round(p[2], 0))
        if key not in node_at:
            node_at[key] = len(node_at)
            graph.pos[node_at[key]] = p
        return node_at[key]

    splits = collections.defaultdict(list)  # (pi, si) -> [(t, point)]
    canon = {}  # merged endpoints: (pi, 0|-1) -> point

    def near_segments(p):
        gx, gy = int(p[0] // cell), int(p[1] // cell)
        seen = set()
        for dx in (-1, 0, 1):
            for dy in (-1, 0, 1):
                for item in grid.get((gx + dx, gy + dy), ()):
                    if item not in seen:
                        seen.add(item)
                        yield item

    # Road ends onto other roads.
    for pi, poly in enumerate(polys):
        if not poly["merge"]:
            continue
        for end in (0, -1):
            p = poly["pts"][end]
            best = None
            for qi, si in near_segments(p):
                if qi == pi or not polys[qi]["merge"]:
                    continue
                a, b = polys[qi]["pts"][si], polys[qi]["pts"][si + 1]
                abx, aby = b[0] - a[0], b[1] - a[1]
                ll = abx * abx + aby * aby
                t = 0.0 if ll < 1e-9 else max(0.0, min(1.0, ((p[0] - a[0]) * abx + (p[1] - a[1]) * aby) / ll))
                q = (a[0] + abx * t, a[1] + aby * t, a[2] + (b[2] - a[2]) * t, a[3] + (b[3] - a[3]) * t)
                d = flat(p, q)
                if abs(q[2] - p[2]) <= 4 and d <= max(p[3], q[3]) * 0.5 + 1 and (best is None or d < best[0]):
                    best = (d, qi, si, t, q)
            if best:
                _, qi, si, t, q = best
                splits[(qi, si)].append((t, q))
                canon[(pi, end)] = q

    # Centrelines crossing mid-segment.
    for key, items in grid.items():
        for i, (pi, si) in enumerate(items):
            for qi, sj in items[i + 1:]:
                if pi >= qi or not (polys[pi]["merge"] and polys[qi]["merge"]):
                    continue
                a, b = polys[pi]["pts"][si], polys[pi]["pts"][si + 1]
                c, d = polys[qi]["pts"][sj], polys[qi]["pts"][sj + 1]
                den = (b[0] - a[0]) * (d[1] - c[1]) - (b[1] - a[1]) * (d[0] - c[0])
                if abs(den) < 1e-9:
                    continue
                t = ((c[0] - a[0]) * (d[1] - c[1]) - (c[1] - a[1]) * (d[0] - c[0])) / den
                u = ((c[0] - a[0]) * (b[1] - a[1]) - (c[1] - a[1]) * (b[0] - a[0])) / den
                if not (0.02 < t < 0.98 and 0.02 < u < 0.98):
                    continue
                pa = (a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t, a[3])
                zc = c[2] + (d[2] - c[2]) * u
                if abs(pa[2] - zc) > 4:
                    continue
                splits[(pi, si)].append((t, pa))
                splits[(qi, sj)].append((u, pa))

    for pi, poly in enumerate(polys):
        pts = list(poly["pts"])
        if (pi, 0) in canon:
            pts[0] = canon[(pi, 0)]
        if (pi, -1) in canon:
            pts[-1] = canon[(pi, -1)]
        attrs = {k: poly[k] for k in ("drv", "type", "oneWay", "speed", "name")}
        prev = node_for(pts[0])
        for si in range(len(pts) - 1):
            width = (pts[si][3] + pts[si + 1][3]) / 2
            seen_points = sorted(splits.get((pi, si), []), key=lambda item: item[0])
            for _, point in seen_points:
                current = node_for(point)
                graph.add_edge(prev, current, dict(attrs, width=width))
                prev = current
            current = node_for(pts[si + 1])
            graph.add_edge(prev, current, dict(attrs, width=width))
            prev = current
    return graph


def exits_of(graph, node, came_from):
    """Exits leaving node for an approach from came_from: (other, edge, bearing)."""
    approach = heading(graph.pos[came_from], graph.pos[node])
    out = []
    for other, edge in graph.adj[node]:
        if other == came_from:
            continue
        if edge["oneWay"] and edge["a"] != node:
            continue
        out.append((other, edge, norm(heading(graph.pos[node], graph.pos[other]) - approach)))
    return out


def analyse(level):
    graph = build(level["roads"])
    if not graph.edges:
        return None
    km = sum(e["length"] for e in graph.edges) / 1000
    stats = {"km": km, "nodes": len(graph.pos), "edges": len(graph.edges)}
    drv = [e["drv"] for e in graph.edges for _ in range(max(1, round(e["length"] / 20)))]
    stats["drv_values"] = collections.Counter(round(v, 2) for v in drv).most_common(5)
    weigh = lambda test: sum(e["length"] for e in graph.edges if test(e)) / 1000
    stats["private_km"] = weigh(lambda e: e["type"] == "private")
    stats["oneway_km"] = weigh(lambda e: e["oneWay"])
    stats["speed_km"] = weigh(lambda e: e["speed"] not in (None, 0, "0", ""))
    stats["types"] = collections.Counter(e["type"] for e in graph.edges).most_common(4)
    widths = sorted(e["width"] for e in graph.edges)
    stats["width_median"] = statistics.median(widths)
    # Approaches at branching nodes.
    approaches = shapes = 0
    counts = collections.Counter()
    side_ratio_drv, side_ratio_width = [], []
    for node, links in graph.adj.items():
        if len(links) < 3:
            continue
        for came_from, incoming in links:
            if incoming["oneWay"] and incoming["b"] != node:
                continue
            exits = exits_of(graph, node, came_from)
            usable = [x for x in exits if abs(x[2]) <= 150]
            if len(usable) < 2:
                continue
            approaches += 1
            # Is there an exit carrying straight on?
            straight = min(usable, key=lambda x: abs(x[2]))
            s = abs(straight[2])
            counts["straight<=10" if s <= 10 else "straight<=30" if s <= 30 else "no straight"] += 1
            near = [x for x in usable if abs(x[2]) <= 45]
            if len(near) >= 2:
                counts["shallow_pair"] += 1
                pair = sorted(near, key=lambda x: abs(x[2]))
                if abs(pair[0][2]) <= 10:
                    counts["shallow_pair_one_straight"] += 1
                else:
                    counts["true_fork"] += 1
            for x in usable:
                if x is straight:
                    continue
                side_ratio_drv.append(x[1]["drv"] / max(0.01, straight[1]["drv"]))
                side_ratio_width.append(x[1]["width"] / max(0.5, straight[1]["width"]))
                lesser = x[1]["type"] == "private" and straight[1]["type"] != "private"
                lesser = lesser or x[1]["drv"] < 0.7 * straight[1]["drv"] or x[1]["width"] < 0.7 * straight[1]["width"]
                counts["side_lesser" if lesser else "side_equal"] += 1
    stats["approaches"] = approaches
    stats["per_km"] = approaches / max(0.01, km)
    stats["counts"] = counts
    stats["short_links"] = sum(1 for e in graph.edges if e["length"] <= 35
                               and len(graph.adj[e["a"]]) >= 3 and len(graph.adj[e["b"]]) >= 3)
    return stats


def main():
    rows = []
    for path in sorted(pathlib.Path(sys.argv[1]).glob("*.json")):
        level = json.loads(path.read_text(encoding="utf-8"))
        stats = analyse(level)
        if stats:
            rows.append((level["level"], stats))
    print(f"{'level':24}{'km':>7}{'nodes':>7}{'priv km':>9}{'1way km':>9}{'speed km':>9}{'w med':>7}{'appr/km':>9}")
    for name, s in rows:
        print(f"{name:24}{s['km']:7.0f}{s['nodes']:7}{s['private_km']:9.1f}{s['oneway_km']:9.1f}{s['speed_km']:9.1f}"
              f"{s['width_median']:7.1f}{s['per_km']:9.1f}")
    print()
    print(f"{'level':24}{'st<=10':>8}{'st<=30':>8}{'nostr':>7}{'shallow':>9}{'1 straight':>11}{'true fork':>10}{'side less':>10}{'side eq':>9}{'short':>7}")
    for name, s in rows:
        c = s["counts"]
        print(f"{name:24}{c['straight<=10']:8}{c['straight<=30']:8}{c['no straight']:7}{c['shallow_pair']:9}"
              f"{c['shallow_pair_one_straight']:11}{c['true_fork']:10}{c['side_lesser']:10}{c['side_equal']:9}{s['short_links']:7}")
    print()
    for name, s in rows:
        print(f"{name:24} types={s['types']}  drv={s['drv_values']}")


main()

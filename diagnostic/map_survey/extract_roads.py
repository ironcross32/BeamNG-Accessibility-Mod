"""Pull only the DecalRoad objects and level info out of each map archive.

Reads the zips in place and writes one JSON per level: no textures, no 12 GB unpack.
usage: python -I extract_roads.py <levels_dir> <out_dir>
"""
import json
import pathlib
import sys
import zipfile

levels, out = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])
out.mkdir(parents=True, exist_ok=True)
for archive in sorted(levels.glob("*.zip")):
    roads, info, scene_files, waypoints = [], None, 0, 0
    with zipfile.ZipFile(archive) as z:
        for name in z.namelist():
            low = name.lower()
            if low.endswith("/info.json") and low.count("/") == 2 and info is None:
                try:
                    info = json.loads(z.read(name).decode("utf-8", "ignore"))
                except ValueError:
                    pass
            if not low.endswith("items.level.json"):
                continue
            scene_files += 1
            for line in z.read(name).decode("utf-8", "ignore").splitlines():
                if '"DecalRoad"' not in line and '"BeamNGWaypoint"' not in line:
                    continue
                try:
                    obj = json.loads(line)
                except ValueError:
                    continue
                if obj.get("class") == "BeamNGWaypoint":
                    waypoints += 1
                elif obj.get("class") == "DecalRoad":
                    keep = {k: obj.get(k) for k in (
                        "name", "drivability", "oneWay", "gatedRoad", "type", "speedLimit", "flipDirection",
                        "autoJunction", "autoLanes", "lanesLeft", "lanesRight", "hiddenInNavi", "nodes",
                    ) if k in obj}
                    if keep.get("nodes"):
                        roads.append(keep)
    title = (info or {}).get("title", archive.stem)
    (out / f"{archive.stem}.json").write_text(json.dumps({"level": archive.stem, "title": title,
        "roads": roads, "waypoints": waypoints}), encoding="utf-8")
    drivable = sum(1 for r in roads if (r.get("drivability") or 0) > 0)
    print(f"{archive.stem:32} scene files {scene_files:4}  decal roads {len(roads):6}  drivable {drivable:6}  waypoints {waypoints}")

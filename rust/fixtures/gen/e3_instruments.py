"""Golden cases for the E3 instruments (beam_engine::listeners::{dock_phrases, cannon,
lineup, trailer, wheel_report, route_beacon, terrain_scan, implement}).

    uv run python rust/fixtures/gen/e3_instruments.py

Two halves:

* ``phrases``: every pure phrasing / parsing function in beamtel.py's implement, dock, ramp
  and cannon section, over a table of inputs crossing each threshold, in both unit systems.
* ``listeners``: the REAL beamtel listeners driven datagram by datagram. The socket, both
  clocks, ``say``, the audio controller and the command sender are replaced with recorders;
  after every datagram the calls it made and the globals it writes are captured.
"""

from __future__ import annotations

import copy
import math
import socket as real_socket
import types

from _beamtel import beamtel
from _common import fbits, write_fixture

b = beamtel
NAN = math.nan
INF = math.inf


def enc(v):
    if v is None or isinstance(v, (bool, str)):
        return v
    if isinstance(v, int):
        return v
    if isinstance(v, float):
        return fbits(v)
    if isinstance(v, (list, tuple)):
        return [enc(x) for x in v]
    if isinstance(v, dict):
        return {str(k): enc(x) for k, x in v.items()}
    raise TypeError(type(v).__name__)


UNITS = ("imperial", "metric")


def per_units(fn, *args):
    out = {}
    for u in UNITS:
        b.UNITS_MODE = u
        try:
            out[u] = {"ok": fn(*args)}
        except Exception as exc:  # noqa: BLE001
            out[u] = {"err": type(exc).__name__}
    b.UNITS_MODE = "imperial"
    return out


# ---------------------------------------------------------------------------------------
#  Phrases
# ---------------------------------------------------------------------------------------
def impl_dock(**kw):
    d = {
        "name": "pallet", "range": 1.0, "lateral": 0.0, "vertical": 0.0, "idx": 1,
        "count": 3, "kind": "GAP", "lo_z": 0.0, "hi_z": 0.15, "yaw": 0.0, "manual": False,
        "impl_word": "Forks", "entry_theta": 0.0, "entry_depth": 1.2, "mode": "IMPL",
        "margin": None, "acquire": False,
    }
    d.update(kw)
    return d


def ramp_dock(**kw):
    d = impl_dock(name="large cannon", range=8.0, idx=0, count=0, kind="RAMP", hi_z=0.0,
                  impl_word="Bucket", entry_depth=-1.0, mode="RAMP", margin=0.9)
    d.update(kw)
    return d


DOCKS = [
    impl_dock(), impl_dock(vertical=0.30), impl_dock(vertical=-0.30),
    impl_dock(vertical=0.04, lateral=-0.04), impl_dock(vertical=0.05, lateral=0.05),
    impl_dock(lateral=0.4), impl_dock(lateral=-0.4), impl_dock(yaw=7.99), impl_dock(yaw=8.0),
    impl_dock(yaw=-20.0, manual=True), impl_dock(range=-0.02), impl_dock(entry_depth=0.15),
    impl_dock(entry_depth=0.0), impl_dock(entry_depth=0.4), impl_dock(entry_depth=-1.0),
    impl_dock(entry_depth=None, entry_theta=None), impl_dock(kind="SOLID", idx=5, count=5),
    impl_dock(kind="SOLID", idx=1, count=4), impl_dock(kind="SOLID", idx=2, count=4),
    impl_dock(kind="GAP", idx=3, count=4), impl_dock(lo_z=0.5, hi_z=0.2),
    impl_dock(vertical=NAN, lateral=NAN, range=NAN), impl_dock(yaw=-0.0, range=INF),
    impl_dock(name="Gavril D-Series", range=1.24, lateral=-0.12, vertical=0.22, idx=2,
              count=5, lo_z=0.35, hi_z=0.52, yaw=-14.0, manual=True),
    ramp_dock(), ramp_dock(lateral=0.4), ramp_dock(lateral=-0.4), ramp_dock(lateral=0.15),
    ramp_dock(lateral=0.2), ramp_dock(yaw=12.0), ramp_dock(yaw=-12.0), ramp_dock(yaw=0.49),
    ramp_dock(yaw=0.5), ramp_dock(yaw=-1.5), ramp_dock(yaw=2.5), ramp_dock(margin=0.24),
    ramp_dock(margin=0.25), ramp_dock(margin=0.0), ramp_dock(margin=-0.3),
    ramp_dock(margin=-0.999), ramp_dock(margin=-0.998), ramp_dock(margin=-1.0),
    ramp_dock(margin=None), ramp_dock(entry_theta=4.99), ramp_dock(entry_theta=5.0),
    ramp_dock(entry_theta=-9.0), ramp_dock(entry_theta=-999.0), ramp_dock(entry_theta=None),
    ramp_dock(range=-2.0), ramp_dock(range=25.0), ramp_dock(range=24.0, lateral=7.0),
    ramp_dock(range=-9.0, lateral=4.2, yaw=123.0, margin=-3.1, acquire=True),
    ramp_dock(range=-30.0, lateral=6.0, yaw=100.0, acquire=True),
    ramp_dock(range=10.0, lateral=0.1, yaw=1.0, acquire=True),
    ramp_dock(range=-0.5, lateral=0.0, acquire=True), ramp_dock(range=-0.49, acquire=True),
    ramp_dock(range=0.0, lateral=-8.0, yaw=-170.0, acquire=True, margin=0.1),
    ramp_dock(range=2.1, lateral=1.55, yaw=-11.0, margin=-0.35, entry_theta=9.0),
    ramp_dock(mode="IMPL"), impl_dock(mode="RAMP", margin=None),
]


def shot(**kw):
    s = {"downrange": 180.0, "lateral": 0.0, "apex": 0.0, "flight": 4.0, "ramp_pitch": 20.0,
         "strength": 80, "settled": True, "index": 1, "near_dist": -1.0, "near_name": "",
         "vehicle": "pickup", "stamp": 0.0}
    s.update(kw)
    return s


SHOTS = [
    shot(), shot(lateral=20.0), shot(lateral=-20.0), shot(lateral=3.0), shot(lateral=3.01),
    shot(settled=False, downrange=412.0), shot(apex=14.99), shot(apex=15.0),
    shot(near_name="Ibishu Covet", near_dist=3.2), shot(near_name="Ibishu Covet", near_dist=-1.0),
    shot(near_name="", near_dist=5.0), shot(prev_downrange=120.0, downrange=180.0),
    shot(downrange=120.0, prev_downrange=180.0), shot(downrange=184.99, prev_downrange=180.0),
    shot(downrange=185.0, prev_downrange=180.0), shot(downrange=-40.0, lateral=50.0),
    shot(downrange=300.0), shot(downrange=0.5), shot(downrange=2.5), shot(downrange=NAN),
    shot(downrange=INF, lateral=-INF, apex=INF),
    shot(downrange=240.0, lateral=-31.0, apex=40.0, near_name="Ibishu Covet", near_dist=3.2,
         prev_downrange=305.0),
]

ALIGNS = [
    "OK,large cannon,6.10,0.85", "OK,tilt deck,6.10,-1.00", "OK,dry van,6.10,0.00",
    "OK,dry van,6.10,0.04", "OK,dry van,6.10,0.10", "OK,large cannon,6.10,NA",
    "OK,us semi,6.10,0.85,1.30", "OK,us semi,6.10,0.85,0.95", "OK,us semi,6.10,0.85,0.15",
    "OK,us semi,6.10,0.04,0.15", "OK,us semi,6.10,0.85,NA", "OK,us semi,6.10,0.04,1.30",
    "OK,us semi,6.10,0.85,0.30", "OK,us semi,6.10,0.85,0.29", "OK, ,NA", "OK,", "OK",
    "OK,x", "OK,x,nan,nan,nan", "OK,x, 1_0 ,inf", "FAIL,nearest is Old Cannon, 5 metres, no ramp on it",
    "FAIL,", "WAT", "",
]

RAMP_SELFS = [
    "0.0,1.30;upfit_tilt:1:52;upfit_extendRetract:0:552;upfit_extendRetractFeet:0:22",
    "10.1,0.15;upfit_tilt:100:52;upfit_extendRetract:100:552;upfit_extendRetractFeet:0:22",
    "10.2,0.95;upfit_tilt:100:52;upfit_extendRetract:0:552",
    "0.0,-999.00;upfit_tilt:1:52", "0.0;upfit_tilt:1:52", "0.0;upfit_tilt:34:52",
    "0.0;deck_tilt:5:52;bed_slide:5:52", "-999.0;upfit_tilt:1:52;upfit_extendRetract:0:552",
    "-25.0;", "12.0;", "0.0;broken;upfit_tilt:7:52", "NONE", " none ", "", "wat;x:y:z",
    "0.99", "-0.99", "1.0", "-180.0", "-180.1", "nan", "0.0,nan", "0.0,-900.0", "0.0,0.29",
    "0.0;upfit:1:52;upfit_tilt:2:52", "0.0;a_b:1:52;a_c:x:52;a_d:1:5.2", "0.0;camelCaseHUMPName:50:100",
    "0.0;x_:1:99;x_y:2:100", "0.0;Über_Ärger:3:200;Über_Öl:4:300", "0.0;upfit_tilt: 7 :52",
]

HYD_WORDS = ["extendRetractFeet", "upfit_tilt", "ABCDef", "a__b", "_lead", "camelCase_With_Under",
             "XMLHttp", "x", "", "ÄpfelBaum", "tail_"]

STRIP_NS = [["upfit_tilt"], ["upfit_tilt", "upfit_extendRetract"], ["a_b", "c_d"],
            ["upfit", "upfit_tilt"], ["x_", "x_y"], ["noUnderscore", "noUnderscore"],
            ["Ä_b", "Ä_c"], []]

BEARINGS = [(10.0, 0.0, 0.0), (10.0, 5.0, 0.0), (10.0, -5.0, 0.0), (-9.0, 0.0, 0.0),
            (10.0, 0.0, 30.0), (-9.0, 4.2, 123.0), (0.0, 0.0, 0.0), (0.0, -0.0, 180.0),
            (-30.0, 6.0, 100.0), (1.0, 1.0, -400.0), (NAN, 1.0, 0.0), (INF, 1.0, 0.0)]

ACQUIRES = [(r, l, p) for r in (-0.2, 0.49, 0.5, 12.0) for l in (0.0, 5.9, 6.0, 6.1, 8.9, 9.0, 9.1, -9.1)
            for p in (True, False)]


def phrases():
    out = {}
    out["band_name"] = [[k, i, c, b._band_name(k, i, c)]
                        for k in ("GAP", "SOLID", "RAMP") for i in (0, 1, 2, 4, 5) for c in (1, 4, 5)]
    out["implement_word"] = [[p, b._implement_word(p)] for p in
                             ("bucket", "FORKS_heavy", "grapple_x", "Forklift Grapple", "", "NONE", "blade")]
    out["ramp_bearing_deg"] = [[enc(r), enc(l), enc(y), enc(b._ramp_bearing_deg(r, l, y))]
                               for r, l, y in BEARINGS]
    out["ramp_acquire"] = [[enc(r), enc(l), p, b._ramp_acquire(r, l, p)] for r, l, p in ACQUIRES]
    out["ramp_align_phrase"] = [[p, per_units(b._ramp_align_phrase, p)] for p in ALIGNS]
    out["cannon_shot_phrase"] = [[enc(s), per_units(b._cannon_shot_phrase, s)] for s in SHOTS]
    out["dock_phrase"] = [[enc(d), per_units(b._dock_phrase, d)] for d in DOCKS]
    rs = []
    for p in RAMP_SELFS:
        parsed = b._parse_ramp_self(p)
        rs.append([p, enc(parsed),
                   per_units(b._ramp_self_phrase, parsed) if parsed is not None else None])
    out["ramp_self"] = rs
    out["ramp_hyd_words"] = [[n, b._ramp_hyd_words(n)] for n in HYD_WORDS]
    out["ramp_hyd_strip_namespace"] = [[n, b._ramp_hyd_strip_namespace(n)] for n in STRIP_NS]
    return out


# ---------------------------------------------------------------------------------------
#  Listeners
# ---------------------------------------------------------------------------------------
class Runner:
    def __init__(self):
        self.steps, self.idx, self.served, self.calls, self.out = [], 0, None, [], []
        self.prelude = []
        self.now = 0.0
        self.mono = 0.0
        self.snap = lambda: {}


R = Runner()


class FakeStop:
    def is_set(self):
        if R.served is not None:
            finish_step()
        elif R.idx == 0:
            R.prelude = R.calls
            R.calls = []
        if R.idx >= len(R.steps):
            return True
        begin_step(R.steps[R.idx])
        R.idx += 1
        return False


class FakeSock:
    def bind(self, addr):
        pass

    def settimeout(self, t):
        pass

    def recvfrom(self, n):
        if R.served["data"] is None:
            raise real_socket.timeout()
        return R.served["data"], ("127.0.0.1", 50000)

    def close(self):
        pass


class Audio:
    def __getattr__(self, name):
        def rec(*args, **kwargs):
            R.calls.append([name] + [enc(a) for a in args] + [enc(kwargs[k]) for k in sorted(kwargs)])
        return rec


def rec_say(text, interrupt=True, exclude_from_buffer=False, source="python"):
    R.calls.append(["say", text, bool(interrupt), bool(exclude_from_buffer)])


SETTABLE = {"announce_implement_proximity", "announce_cannon_shot", "dock_mode_active",
            "UNITS_MODE", "_lineup_layer_open", "_lineup_enabled"}


def begin_step(step):
    R.served = step
    R.now = step["now"]
    R.mono = step["mono"]
    for k, v in step.get("set", {}).items():
        assert k in SETTABLE, k
        setattr(b, k, v)


def finish_step():
    step = R.served
    R.served = None
    R.out.append({
        "now": fbits(step["now"]),
        "mono": fbits(step["mono"]),
        "data": None if step["data"] is None else step["data"].hex(),
        "set": step.get("set", {}),
        "calls": R.calls,
        "snap": R.snap(),
    })
    R.calls = []


INITIAL = {
    "last_dock": None, "last_dock_fail": None, "cannon_active": False, "last_dock_name": None,
    "last_ramp_self": None, "last_dock_mode": None, "cannon_kind": "NONE", "last_cannon_aim": None,
    "_implement_word_current": "", "announce_implement_proximity": True, "dock_mode_active": False,
    "last_cannon_shot": None, "cannon_shot_session": [], "announce_cannon_shot": True,
    "last_trailer_deg": 0.0, "last_trailer_id": None, "last_trailer_name": "",
    "last_trailer_stamp": 0.0, "last_wheel_rows": [], "last_wheel_stamp": 0.0,
    "route_dest_x": None, "route_dest_y": None, "route_remaining_m": 0.0,
    "route_beacon_stamp": 0.0, "_last_scan_reply_ts": 0.0, "_last_poi_reply_ts": 0.0,
    "UNITS_MODE": "imperial", "_lineup_geom": None, "_lineup_stamp": 0.0,
    "_lineup_mode": "head_on", "_lineup_offset_m": 3.0, "_lineup_depth_m": 10.0,
    "_lineup_enabled": False, "_lineup_muted": False, "_lineup_layer_open": False,
}


def snapper(names):
    def snap():
        return {n: enc(getattr(b, n)) for n in names}
    return snap


def lineup_snap():
    g = b._lineup_geom
    d = snapper(["_lineup_stamp", "_lineup_mode", "_lineup_offset_m", "_lineup_depth_m",
                 "_lineup_muted"])()
    d["_lineup_geom"] = None if g is None else enc([g.cam_x, g.cam_y, g.cam_fx, g.cam_fy,
                                                    g.nose_x, g.nose_y, g.veh_fx, g.veh_fy])
    return d


def run_listener(name, fn, args, steps, snap_names, initial=None):
    for k, v in INITIAL.items():
        setattr(b, k, copy.deepcopy(v))
    for k, v in (initial or {}).items():
        setattr(b, k, v)
    R.steps, R.idx, R.served, R.calls, R.out, R.prelude = steps, 0, None, [], [], []
    R.snap = snap_names if callable(snap_names) else snapper(snap_names)
    R.now = R.mono = 1_699_999_000.0
    fn(*args, stop_event=FakeStop())
    return {"name": name, "initial": enc(initial or {}), "start_now": fbits(1_699_999_000.0),
            "prelude": R.prelude, "steps": R.out}


class Steps:
    def __init__(self, t=1_700_000_000.0, m=5_000.0):
        self.t, self.m, self.list = t, m, []

    def add(self, data, dt=0.1, **opts):
        self.t += dt
        self.m += dt
        if isinstance(data, str):
            data = data.encode("utf-8")
        self.list.append({"data": data, "now": self.t, "mono": self.m, **opts})
        return self


DOCK_SNAP = ["last_dock", "last_dock_fail", "last_dock_mode", "last_dock_name", "cannon_kind",
             "cannon_active", "last_cannon_aim", "last_ramp_self", "_implement_word_current"]


def implement_scenarios():
    out = []
    ON = {"dock_mode_active": True}

    s = Steps()
    s.add(None).add(None, dt=2.0).add(None, dt=0.5).add(None, dt=1.6)
    s.add("IMPLEMENT:forklift_forks").add(None, dt=3.0)
    out.append(("probe", s.list, None))

    def near(dist, rel="LEVEL", inside="0", name="Gavril D-Series"):
        return f"NEAR:{name},{dist},{rel},{inside},0"

    s = Steps()
    s.add("IMPLEMENT:loader_bucket")
    for d in [5.0, 4.0, 3.5, 2.8, 2.0, 1.5, 2.0, 3.0, 4.0, 5.0, 5.5, 6.0, 6.0, 6.0]:
        s.add(near(d))
    for _ in range(10):
        s.add(near(3.05)).add(near(2.95))
    for _ in range(10):
        s.add(near(4.6)).add(near(4.4))
    for _ in range(6):
        s.add(near(2.0, "BELOW")).add(near(2.0, "LEVEL"))
    for _ in range(6):
        s.add(near(2.0, "BELOW"))
    for _ in range(5):
        s.add(near(0.5, "BELOW", "1"))
    for _ in range(5):
        s.add(near(0.9, "BELOW", "0"))
    for _ in range(6):
        s.add(near(0.5, "ABOVE", "1")).add(near(0.5, "ABOVE", "0"))
    for _ in range(5):
        s.add(near(1.0, "ABOVE", name="Ibishu Covet"))
    s.add(near(1.0, "WEIRD", name="Ibishu Covet"))
    for _ in range(6):
        s.add(near(1.0, "WEIRD", name="Ibishu Covet"))
    for _ in range(5):
        s.add("CLEAR")
    s.add("NEAR:a,b,c").add("NEAR:x, y ,nope,LEVEL,0,0").add(near(2.0, "level", " 1 "))
    s.add(near(1.0), set={"announce_implement_proximity": False})
    s.add(near(1.0), set={"announce_implement_proximity": True})
    s.add("CLEAR").add("CLEAR", dt=0.29).add("CLEAR", dt=0.01)
    s.add(near(NAN)).add(near(1.0e-3))
    out.append(("proximity", s.list, None))

    s = Steps()
    s.add("IMPLEMENT:pallet_forks", set=ON)
    s.add("DOCK:pallet,1.240,-0.120,0.220,2,5,GAP,0.350,0.520,-14.0,1,36.0,0.187")
    s.add("DOCK:pallet,1.240,-0.120,0.220,2,5,GAP,0.350,0.520,-14.0,1")
    s.add("DOCK:pallet, stacked,1.240,-0.120,0.220,2,5,GAP,0.350,0.520,-14.0,1,36.0,0.187")
    s.add("DOCK:pallet,1.0,0.1,0.2,2,5,gap,0.3,0.5,1.0,1,3.0,0.15")
    s.add("DOCK:pallet,1.0,0.1,0.2,2,5,gap,0.3,0.5,1.0,1,3.0,1.2")
    s.add("DOCK:pallet,1.0,0.1,0.2,2,5,gap,0.3,0.5,1.0,1,3.0,0.37")
    s.add("DOCK:pallet,1.0,0.1,0.2,2,5,gap,0.3,0.5,1.0,1,3.0,0.33")
    s.add("DOCK:pallet,1.0,0.1,0.2,2,5,gap,0.3,0.5,1.0,1,3.0,0.39")
    s.add("DOCK:pallet,1.0,0.1,0.2,2,5,gap,0.3,0.5,1.0,1,3.0,0.40")
    s.add("DOCKCLEAR")
    s.add("DOCK:pallet,1.0,0.1,0.2,2,5,gap,0.3,0.5,1.0,1,3.0,1.2")
    s.add("DOCKFAIL: nothing in range ")
    s.add("DOCK:pallet,1.0,0.1,0.2,2,5,gap,0.3,0.5,1.0,1,3.0,-1.0")
    s.add("DOCK:pallet,1.0,0.1,0.2,2,5,gap,0.3,0.5,1.0,1,3.0,nan")
    s.add("DOCK:pallet,x,0.1,0.2,2,5,gap,0.3,0.5,1.0,1,3.0,0.5")
    s.add("DOCK:pallet,1,0.1,0.2,2.5,5,gap,0.3,0.5,1.0,1,3.0,0.5")
    s.add("DOCK:a,b").add("DOCK:")
    s.add("DOCK:large cannon,40.0,10.0,0.0,0,0,RAMP,1.2,1.2,6.0,0,2.0,-1.0,RAMP,0.9")
    s.add("DOCK:large cannon,12.0,6.1,0.0,0,0,RAMP,1.2,1.2,6.0,0,2.0,-1.0,RAMP,0.9")
    s.add("DOCK:large cannon,12.0,5.9,0.0,0,0,RAMP,1.2,1.2,6.0,0,2.0,-1.0,ramp,0.9")
    s.add("DOCK:large cannon,12.0,8.9,0.0,0,0,RAMP,1.2,1.2,6.0,0,-999,-1.0,RAMP,-1")
    s.add("DOCK:large cannon,12.0,9.1,0.0,0,0,RAMP,1.2,1.2,6.0,0,2.0,-1.0,RAMP,0.9")
    s.add("DOCK:Large, Cannon,-9.0,4.2,0.0,0,0,RAMP,1.2,1.2,123.0,0,2.0,-1.0,RAMP,-3.1")
    s.add("DOCK:Large, Cannon,0.4,0.0,0.0,0,0,RAMP,1.2,1.2,0.0,0,2.0,-1.0,RAMP,0.2")
    s.add("DOCK:other,5.0,0.0,0.0,0,0,RAMP,1.2,1.2,0.0,0,2.0,-1.0,WHAT,0.2")
    s.add("DOCK:other,5.0,0.0,0.0,0,0,RAMP,1.2,1.2,0.0,0,2.0,-1.0,RAMP,x")
    s.add("DOCK:large cannon,12.0,1.0,0.0,0,0,RAMP,1.2,1.2,6.0,0,2.0,-1.0,RAMP,0.9",
          set={"dock_mode_active": False})
    s.add("DOCK:pallet,1.0,0.1,0.2,2,5,gap,0.3,0.5,1.0,1,3.0,1.2")
    s.add("DOCK:pallet,1.0,0.1,0.2,2,5,gap,0.3,0.5,1.0,1,3.0,1.2", set=ON)
    s.add("IMPLEMENT: NONE ")
    s.add("DOCK:pallet,1.0,0.1,0.2,2,5,gap,0.3,0.5,1.0,1,3.0,1.2")
    out.append(("dock", s.list, None))

    s = Steps()
    for line in [
        "RAMPALIGN:OK,large cannon,6.10,0.85", "RAMPALIGN:OK,us semi,6.10,0.04,1.30",
        "RAMPALIGN:FAIL,nearest is Old Cannon, 5 metres, no ramp on it", "RAMPALIGN:WAT",
        "CANNON:1", "CANNONAIM:30.0,2.0,31.5,29.0,120.0,55.0,1", "CANNON: old ",
        "CANNONAIM:30.0,2.0,31.5,29.0,120.0,55.0,1", "CANNONAIM:30.0,2.0,31.5,29.0,-1.0,0.0,0",
        "CANNONAIM:30.0,2.0,31.5,29.0,120.0,0.0, 1 ", "CANNONAIM:1,2,3", "CANNONAIM:a,2,3,4,5,6,1",
        "CANNON:large", "CANNON:whatever", "CANNON:OLD", "CANNONAIM:1,2,3,4,5,6,1", "CANNON:",
        "RAMPSELF:10.1,0.15;upfit_tilt:100:52;upfit_extendRetract:100:552",
        "RAMPSELF:NONE", "RAMPSELF:-999;", "RAMPSELF:garbage",
        "SLAM:clear", "SLAM: OVER ", "SLAM:COMMITTED", "SLAM:NONE", "SLAM:weird",
    ]:
        s.add(line)
    s.add("SLAM:COMMITTED", set={"announce_implement_proximity": False})
    s.add("UNKNOWN:thing").add("   ")
    out.append(("cannon_ramp_slam", s.list, None))

    s = Steps()
    s.add("IMPLEMENT:grapple_arm", set={"UNITS_MODE": "metric"})
    s.add("NEAR:hay bale,2.345,ABOVE,0,0").add("RAMPALIGN:OK,large cannon,6.10,0.85")
    out.append(("metric", s.list, None))

    return [run_listener(n, b.implement_listener, (Audio(),), st, DOCK_SNAP, i)
            for n, st, i in out]


def cannon_scenarios():
    s = Steps()
    for line in [
        "SHOT:180.0,0.5,4.0,3.2,20.0,80,1,1,-1.0,,pickup",
        "SHOT:240.0,-31.0,40.0,5.0,22.0,90,1,2,3.2, Ibishu Covet ,pickup",
        "SHOT:300.0,5.0,10.0,5.0,22.0,90,0,3,-1,,pickup",
        "SHOT:305.0,8.0,10.0,5.0,22.0,90,1,4,-1,,pickup,240.0",
        "SHOT:305.0,8.0,10.0,5.0,22.0,90,1,5,-1,,pickup,-1e9",
        "SHOT:100.0,8.0,10.0,5.0,22.0,90,1,6,-1,,pickup,x",
        "SHOT:1,2,3", "SHOT:a,2,3,4,5,6,1,7,8,n,v", "SHOT:1,2,3,4,5,6.5,1,7,8,n,v",
        "NOTSHOT:1,2,3,4,5,6,1,7,8,n,v", "SHOT:-40,0,0,1,1,1,1,8,-1,,car",
    ]:
        s.add(line)
    s.add("SHOT:90,0,0,1,1,1,1,9,-1,,car", set={"announce_cannon_shot": False})
    s.add("SHOT:300.0,0,0,1,1,1,1,10,-1,,car", set={"announce_cannon_shot": True, "UNITS_MODE": "metric"})
    snap = ["last_cannon_shot", "cannon_shot_session"]
    return [run_listener("cannon", b.cannon_shot_listener, (), s.list, snap)]


def trailer_scenarios():
    s = Steps()
    for line in ["TRAILER:12.5,42,Dry Van", "TRAILER:13.0,42,Dry Van", "TRAILER:-3.0,43",
                 "TRAILER:CLEAR", "TRAILER:CLEAR", "TRAILER:1", "TRAILER:x,1", "TRAILER:1,2.0",
                 "TRAILER:1, 7 , padded ", "OTHER:1,2", "TRAILER:nan,8,", "TRAILER:CLEAR "]:
        s.add(line)
    snap = ["last_trailer_deg", "last_trailer_id", "last_trailer_name", "last_trailer_stamp"]
    return [run_listener("trailer", b.trailer_angle_listener, (), s.list, snap)]


def route_scenarios():
    s = Steps()
    for line in ["ROUTE:1200.5,-340.25,61.0,4821.5", "ROUTE:1201.5,-340.25,61.0,4800.0",
                 "ROUTE:CLEAR", "ROUTE:CLEAR", "ROUTE:1,2,3", "ROUTE:1,2,nan,4", "ROUTE:0,0,0,-5",
                 "OTHER:1,2,3,4", " ROUTE: CLEAR ", "ROUTE:a,b,c,d"]:
        s.add(line)
    snap = ["route_dest_x", "route_dest_y", "route_remaining_m", "route_beacon_stamp"]
    return [run_listener("route", b.route_beacon_listener, (), s.list, snap)]


def wheel_scenarios():
    s = Steps()
    pkt = ("WHEELS\nV,0,101,us_semi,0\nW,1.8,0.9,0.3,100.0,250.0,0\nW,1.8,-0.9,0.3,101.5,,0\n"
           "W,-2.0,0.9,0.3,,180.0,1\nW,-2.0,-0.9,0.3,90.0,170.0,0\n"
           "V,1,202,dryvan,0\nW,-5.0,1.0,0.3,95.0,,0\nW,-5.0,-1.0,0.3,96.0,,0\n")
    s.add(pkt).add("NOT WHEELS").add("WHEELS\n").add("  WHEELS\nV,0,1,car,0\nW,1,1,0.2,30,40,0\n")
    snap = ["last_wheel_rows", "last_wheel_stamp"]
    return [run_listener("wheels", b.wheel_report_listener, (), s.list, snap)]


def terrain_scenarios():
    s = Steps()
    lines = [
        "SCAN,1,2,3,100.0,24\nS,-90.0,0.0,1.5:d,~,-0.5_2.0,3:i,4:zz,~:g\nS,90.0,7\n"
        "O,10.0,20.0,1.0,p\nO,-5.0,30.0,0.0\nP,0.0,50.0,2.0\nX,1,2\nEND\nS,0,1",
        "SCAN,1,2,3,40.0,24\nS,0.0\nO,1,2\nP,1,2\n",
        "SCAN,1,2,3,40.0\nS,0.0,1",
        "SCAN,1,2,3,x,24\nS,0.0,1",
        "SCAN,1,2,3,40.0,24\nS,0.0,1,bad",
        "SCAN,1,2,3,40.0,24\nS,0.0,1_x",
        "FAIL,no terrain", "FAIL",
        'POI:{"name":"Gas Station","description":"Fuel here","distance":120.0,"bearing":30.0,"radius":5.0}',
        "POI:not json", "POI_FAIL: no level ", "POI_FAIL:",
        "GARBAGE",
    ]
    for line in lines:
        s.add(line)
    s.add('POI:{"name":"Gas Station","distance":3.0,"bearing":-170.0}', set={"UNITS_MODE": "metric"})
    snap = ["_last_scan_reply_ts", "_last_poi_reply_ts"]
    return [run_listener("terrain", b.terrain_scan_listener, (Audio(),), s.list, snap)]


def lineup_scenarios():
    s = Steps()
    s.add("LINEUP:OFF", set={"_lineup_enabled": True})
    s.add("LINEUP:0,0,0,1,10,-20,0,1")
    s.add("LINEUP:0,0,0,1,10,-20,0.5,0.8,extra")
    s.add("LINEUP:0,0,0,1,3,-20,0,1")
    s.add("LINEUP:0,0,0,1,0,20,0,1")
    s.add("LINEUP:1,2,3")
    s.add("LINEUP:nan,0,0,1,0,0,0,1")
    s.add("  LINEUP:0,0,0,0,0,0,0,1  ")
    s.add("LINEUP:OFF")
    s.add("LINEUP:0,0,0,1,10,-20,0,1", set={"_lineup_enabled": False})
    return [run_listener("lineup", b.camera_lineup_listener, (Audio(),), s.list, lineup_snap)]


def lineup_keys():
    """``_lineup_on_key`` with the layer open, each key followed by the push it makes."""
    for k, v in INITIAL.items():
        setattr(b, k, copy.deepcopy(v))
    real_ref, real_mono = b.audio_controller_ref, b.time
    b.audio_controller_ref = Audio()
    b._lineup_enabled = True
    b._lineup_layer_open = True
    b._lineup_geom = b.camera_lineup.parse_packet("LINEUP:0,0,0,1,10,-20,0,1")
    b._lineup_stamp = 5000.0
    out = []
    keys = (["t", "x", "up", "up", "t", "x", "down", "left", "right", "m", "t", "m", "q"]
            + ["left"] * 4 + ["t"] + ["right"] * 60 + ["t", "x"] + ["down"] * 15 + ["t"])
    for i, key in enumerate(keys):
        units = "metric" if i % 7 == 3 else "imperial"
        b.UNITS_MODE = units
        mono = 5000.2 if i < 20 else 5001.0
        b.time = types.SimpleNamespace(monotonic=lambda m=mono: m, time=lambda: 0.0)
        R.calls = []
        b._lineup_on_key(types.SimpleNamespace(name=key.upper() if i == 5 else key))
        out.append({"key": key.upper() if i == 5 else key, "units": units, "mono": fbits(mono),
                    "calls": R.calls, "snap": lineup_snap()})
    b.audio_controller_ref, b.time = real_ref, real_mono
    b.UNITS_MODE = "imperial"
    return out


real = {}


def install():
    real.update(say=b.say, time=b.time, socket=b.socket, send=b._send_implement_cmd,
                unhook=b._unhook_suppressed)
    b.say = rec_say
    b.time = types.SimpleNamespace(time=lambda: R.now, monotonic=lambda: R.mono,
                                   sleep=lambda s: None)
    b.socket = types.SimpleNamespace(socket=lambda *a, **k: FakeSock(),
                                     AF_INET=real_socket.AF_INET,
                                     SOCK_DGRAM=real_socket.SOCK_DGRAM,
                                     timeout=real_socket.timeout)
    b._send_implement_cmd = lambda cmd: R.calls.append(["send_implement_cmd", cmd])
    b._unhook_suppressed = lambda hooks: hooks.clear()


def uninstall():
    b.say, b.time, b.socket = real["say"], real["time"], real["socket"]
    b._send_implement_cmd, b._unhook_suppressed = real["send"], real["unhook"]


def main():
    data = {"phrases": phrases()}
    install()
    try:
        data["listeners"] = (implement_scenarios() + cannon_scenarios() + trailer_scenarios()
                             + route_scenarios() + wheel_scenarios() + terrain_scenarios()
                             + lineup_scenarios())
        data["lineup_keys"] = lineup_keys()
    finally:
        uninstall()
    write_fixture("e3_instruments", data)


if __name__ == "__main__":
    main()

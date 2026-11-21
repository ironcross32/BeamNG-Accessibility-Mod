"""Golden cases for the MCP server's camera chunks (mcp_server._cam_place_chunk and the
two fixed chunks), which are composed Lua and so must match the Python byte for byte.

    uv run python rust/fixtures/gen/mcp_camera.py
"""

from __future__ import annotations

from _common import outcome, write_fixture

import mcp_server as ms

PLACE_CASES = [
    # each addressing form
    {"x": 10, "y": 20, "z": 5, "yaw_deg": 90, "pitch_deg": -10},
    {"x": 1.5, "y": -2.25, "z": 1e-7, "roll_deg": 3},
    {"x": "4", "y": True, "z": 0},
    {"rel_fwd_m": -8, "rel_up_m": 3, "look_at": 1234},
    {"veh_id": 7, "rel_fwd_m": 5, "rel_right_m": 2},
    {"veh_id": "12", "rel_fwd_m": 1},
    {"veh_id": "", "rel_up_m": 1},
    {"veh_id": 3.9, "rel_up_m": 1},
    {"orbit_distance_m": 12, "orbit_azimuth_deg": 135, "orbit_elevation_deg": 25},
    {"orbit_distance_m": 1e20},
    {"x": 1, "y": 2, "z": 3, "look_at": [4, 5, 6]},
    {"x": 1, "y": 2, "z": 3, "look_at": "77"},
    {"x": 1, "y": 2, "z": 3, "look_at": 7.6},
    {"x": 1, "y": 2, "z": 3, "look_at": None},
    # refusals
    {},
    {"yaw_deg": 4},
    {"x": 1, "y": 2, "z": 3, "rel_fwd_m": 4},
    {"rel_fwd_m": 1, "orbit_distance_m": 4},
    {"x": 1, "y": 2},
    {"x": 1, "y": None, "z": 3},
    {"x": "abc", "y": 2, "z": 3},
    {"x": [1], "y": 2, "z": 3},
    {"x": "nan", "y": 2, "z": 3},
    {"x": "inf", "y": 2, "z": 3},
    {"x": 1, "y": 2, "z": 3, "yaw_deg": None},
    {"x": 1, "y": 2, "z": 3, "pitch_deg": "up"},
    {"rel_fwd_m": None},
    {"veh_id": "car", "rel_fwd_m": 1},
    {"veh_id": [1], "rel_fwd_m": 1},
    {"orbit_azimuth_deg": 10},
    {"orbit_distance_m": 0},
    {"orbit_distance_m": -3},
    {"orbit_distance_m": 5, "orbit_elevation_deg": None},
    {"x": 1, "y": 2, "z": 3, "look_at": [1, 2]},
    {"x": 1, "y": 2, "z": 3, "look_at": [1, "b", 3]},
    {"x": 1, "y": 2, "z": 3, "look_at": "car"},
    {"x": 1, "y": 2, "z": 3, "look_at": {"id": 1}},
]


def main():
    write_fixture(
        "mcp_camera",
        {
            "read_lua": ms._CAM_READ_LUA,
            "list_lua": ms._CAM_LIST_LUA,
            "place": [{"args": a, "result": outcome(ms._cam_place_chunk, a)} for a in PLACE_CASES],
        },
    )


if __name__ == "__main__":
    main()

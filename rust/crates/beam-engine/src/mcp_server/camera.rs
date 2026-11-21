//! `camera_control` and `screenshot`: the camera chunks (`_CAM_*`, `_cam_place_chunk`)
//! and the two tools built on them.
//!
//! NOT ONE PERCENT SIGN appears in any chunk below. The Python routes these strings
//! through its own `%` formatting elsewhere, and the modulo operator is the one Lua
//! construct that would make a chunk unsafe to route that way. Hence `n360()`.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

use beam_base::pyfmt::py_float_str;
use beam_road::pyjson::{self, Dict, PyErr, Value as PyJson, py_max, py_min, take_chars};
use beam_road::pytime::DateTime;

use super::tools::{aget, aget_or, aget_val, strip_or};
use super::{
    CAM_SETTLE_MS_DEFAULT, CAM_SETTLE_MS_MAX, CAM_YAW_OFFSET_DEG, CAMERA_ACTIONS, DEPS,
    EXEC_TIMEOUT_GE, MAX_DATAGRAM_CONTENT, SHOT_SETTLE_MS_MAX, TResult, ToolFail,
    console_unreachable, exec_ge, require_world,
};
use crate::convert::json_to_pyjson;
use crate::timer::sleep;

/// Read the pose back the way cameraInfo.lua reads it, so the two can never disagree.
/// `n360` is applied AFTER the rounding as well as before it: a yaw of 0 arrives from
/// atan2 as 359.9999997, which `r3` rounds to a clean 360.0 -- a value that does not exist
/// in a 0..360 convention, and one an agent testing `yaw_deg == 0` would miss.
pub const CAM_READ_LUA: &str = concat!(
    "local p=core_camera.getPosition() local f=core_camera.getForward()",
    " local function n360(a) a=math.fmod(a,360) if a<0 then a=a+360 end return a end",
    " local function r3(v) return math.floor(v*1000+0.5)/1000 end",
    " local y,pi=0,0 local l=math.sqrt(f.x*f.x+f.y*f.y+f.z*f.z)",
    " if l>1e-9 then y=n360(math.deg(math.atan2(-f.x,-f.y))) pi=math.deg(math.asin(f.z/l)) end",
    " local ro=0 local ok,t=pcall(core_camera.getYawPitchRoll)",
    " if ok and type(t)=='table' and t.rollDeg then ro=t.rollDeg end",
    " return jsonEncode({mode=(core_camera.getActiveCamName()) or 'unknown',",
    "is_free=(commands.isFreeCamera() and true or false),",
    "pos={r3(p.x),r3(p.y),r3(p.z)},yaw_deg=n360(r3(y)),pitch_deg=r3(pi),roll_deg=r3(ro),",
    "player_veh_id=be:getPlayerVehicleID(0)})",
);

pub const CAM_LIST_LUA: &str = concat!(
    "local g,v={},{} local ok,gc=pcall(core_camera.getGlobalCameras)",
    " if ok and type(gc)=='table' then for k,_ in pairs(gc) do g[#g+1]=k end end",
    " local vid=be:getPlayerVehicleID(0)",
    " local ok2,vc=pcall(core_camera.getCameraDataById,vid)",
    " if ok2 and type(vc)=='table' then for k,_ in pairs(vc) do v[#v+1]=k end end",
    " table.sort(g) table.sort(v)",
    " return jsonEncode({global_cameras=g,vehicle_cameras=v,player_veh_id=vid,",
    "active=(core_camera.getActiveCamName()) or 'unknown'})",
);

/// Entering the free camera goes through `commands.setFreeCamera()` rather than a bare
/// `setByName(0,'free')`: it seeds the free cam at the camera's current pos/rot
/// (commands.lua:38), so there is no visible jump before the placement lands.
const CAM_ENSURE_FREE: &str = "if not commands.isFreeCamera() then commands.setFreeCamera() end ";

const N360_LUA: &str =
    "local function n360(a) a=math.fmod(a,360) if a<0 then a=a+360 end return a end ";

/// `"%r" % x` of a tool argument.
fn arg_repr(args: &PyJson, key: &str) -> TResult<String> {
    Ok(aget_val(args, key)?.py_repr())
}

/// `key in args`.
fn has_key(args: &PyJson, key: &str) -> bool {
    args.as_dict().is_some_and(|d| d.contains_key(key))
}

/// `_cam_num(args, key, default, required)`. `Ok(None)` is the Python's `None` (an
/// explicit `null`, or a missing key with no default).
fn cam_num(args: &PyJson, key: &str, default: Option<f64>, required: bool) -> TResult<Option<f64>> {
    let val = match aget(args, key)? {
        Some(v) => v.clone(),
        None => default.map_or(PyJson::Null, PyJson::Float),
    };
    if val.is_null() {
        if required {
            return Err(ToolFail::tool(format!("`{key}` is required")));
        }
        return Ok(None);
    }
    let f = match val.py_float() {
        Ok(f) => f,
        Err(PyErr::TypeError(_) | PyErr::ValueError(_)) => {
            return Err(ToolFail::tool(format!(
                "`{key}` must be a number, got {}",
                arg_repr(args, key)?
            )));
        }
        Err(e) => return Err(e.into()),
    };
    if !f.is_finite() {
        return Err(ToolFail::tool(format!("`{key}` must be a finite number")));
    }
    Ok(Some(f))
}

/// `repr(float(val))`. `None` fails as `float(None)` does.
fn lua_num(val: Option<f64>) -> TResult<String> {
    match val {
        Some(f) => Ok(py_float_str(f)),
        None => Err(PyErr::TypeError(
            "float() argument must be a string or a real number, not 'NoneType'".to_string(),
        )
        .into()),
    }
}

/// `_cam_veh_expr(args)`: Lua binding `__v` to the vehicle a relative/orbit placement is
/// measured against.
fn cam_veh_expr(args: &PyJson) -> TResult<String> {
    let vid = aget_val(args, "veh_id")?;
    if vid.is_null() || vid.eq_str("") {
        return Ok("local __v=be:getPlayerVehicle(0)".to_string());
    }
    let n = match vid.py_int() {
        Ok(n) => n,
        Err(PyErr::TypeError(_) | PyErr::ValueError(_)) => {
            return Err(ToolFail::tool(format!(
                "`veh_id` must be an integer vehicle id, got {}",
                vid.py_repr()
            )));
        }
        Err(e) => return Err(e.into()),
    };
    Ok(format!("local __v=be:getObjectByID({n})"))
}

/// The chunk's result as text, `res.get("result") or "unknown error"` concatenated
/// onto a `str`.
fn result_or_unknown(res: &Dict) -> TResult<String> {
    match res.get("result") {
        Some(v) if v.truthy() => match v {
            PyJson::Str(s) => Ok(s.clone()),
            other => Err(PyErr::TypeError(format!(
                "can only concatenate str (not \"{}\") to str",
                other.type_name()
            ))
            .into()),
        },
        _ => Ok("unknown error".to_string()),
    }
}

/// `json.loads(res.get("result") or "")`, failing as `label + " was not JSON: ..."`.
fn result_json(res: &Dict, label: &str) -> TResult<PyJson> {
    let raw = res.get("result").cloned().unwrap_or(PyJson::Null);
    let text = match &raw {
        v if !v.truthy() => String::new(),
        PyJson::Str(s) => s.clone(),
        other => {
            return Err(PyErr::TypeError(format!(
                "the JSON object must be str, bytes or bytearray, not {}",
                other.type_name()
            ))
            .into());
        }
    };
    pyjson::loads(&text).map_err(|_| {
        ToolFail::tool(format!(
            "{label} was not JSON: {}",
            take_chars(&raw.py_repr(), 400)
        ))
    })
}

/// `out["action"] = action` on whatever the chunk returned.
fn set_action(mut out: PyJson, action: &str) -> TResult<PyJson> {
    match &mut out {
        PyJson::Dict(d) => {
            d.insert("action".into(), PyJson::Str(action.to_string()));
            Ok(out)
        }
        PyJson::List(_) => Err(PyErr::TypeError(
            "list indices must be integers or slices, not str".to_string(),
        )
        .into()),
        other => Err(PyErr::TypeError(format!(
            "'{}' object does not support item assignment",
            other.type_name()
        ))
        .into()),
    }
}

fn timed_out(res: &Dict) -> bool {
    res.get("timed_out").is_some_and(PyJson::truthy)
}

fn ok(res: &Dict) -> bool {
    res.get("ok").is_some_and(PyJson::truthy)
}

/// `_cam_read(settle_ms)`: run the readback chunk and return it as a dict.
fn cam_read(settle_ms: &PyJson) -> TResult<PyJson> {
    if settle_ms.truthy() {
        let ms = settle_ms.py_float()?;
        sleep(py_max(0.0, py_min(ms, CAM_SETTLE_MS_MAX)) / 1000.0);
    }
    let res = exec_ge(CAM_READ_LUA)?;
    if timed_out(&res) {
        return Err(console_unreachable(EXEC_TIMEOUT_GE));
    }
    if !ok(&res) {
        return Err(ToolFail::tool(format!(
            "camera readback failed: {}",
            result_or_unknown(&res)?
        )));
    }
    result_json(&res, "camera readback")
}

/// `_cam_run(chunk)`.
fn cam_run(chunk: &str) -> TResult<Dict> {
    let res = exec_ge(chunk)?;
    if timed_out(&res) {
        return Err(console_unreachable(EXEC_TIMEOUT_GE));
    }
    if !ok(&res) {
        return Err(ToolFail::tool(format!(
            "camera command failed: {}",
            result_or_unknown(&res)?
        )));
    }
    Ok(res)
}

/// `_cam_place_chunk(args)`: the free-camera placement chunk for whichever addressing
/// mode was given.
pub fn cam_place_chunk(args: &PyJson) -> TResult<String> {
    let has_abs = ["x", "y", "z"].iter().any(|k| has_key(args, k));
    let has_rel = ["rel_fwd_m", "rel_right_m", "rel_up_m"]
        .iter()
        .any(|k| has_key(args, k));
    let has_orbit = [
        "orbit_azimuth_deg",
        "orbit_elevation_deg",
        "orbit_distance_m",
    ]
    .iter()
    .any(|k| has_key(args, k));
    if [has_abs, has_rel, has_orbit].iter().filter(|b| **b).count() > 1 {
        return Err(ToolFail::tool(
            "give exactly one position form: absolute (x/y/z), relative \
             (rel_fwd_m/rel_right_m/rel_up_m) or orbit (orbit_*)",
        ));
    }

    let mut tgt_default = "";
    let pos_lua = if has_abs {
        let x = cam_num(args, "x", None, true)?;
        let y = cam_num(args, "y", None, true)?;
        let z = cam_num(args, "z", None, true)?;
        format!(
            "local __p=vec3({},{},{})",
            lua_num(x)?,
            lua_num(y)?,
            lua_num(z)?
        )
    } else if has_rel {
        let f = cam_num(args, "rel_fwd_m", Some(0.0), false)?;
        let r = cam_num(args, "rel_right_m", Some(0.0), false)?;
        let u = cam_num(args, "rel_up_m", Some(0.0), false)?;
        // __r is the vehicle's RIGHT: fwd:cross(up), deliberately the mirror of the
        // mod-wide up:cross(fwd) positive-is-LEFT rule. That rule governs bearings
        // reported to a driver; this is a placement parameter literally named "right".
        let veh = cam_veh_expr(args)?;
        format!(
            "{veh} if not __v then error('no such vehicle for a relative placement',0) end \
             local __vp=__v:getPosition() local __f=__v:getDirectionVector() \
             local __u=__v:getDirectionVectorUp() local __r=__f:cross(__u) \
             local __p=vec3(__vp.x,__vp.y,__vp.z)+__f*{}+__r*{}+__u*{}",
            lua_num(f)?,
            lua_num(r)?,
            lua_num(u)?
        )
    } else if has_orbit {
        let az = cam_num(args, "orbit_azimuth_deg", Some(0.0), false)?;
        let el = cam_num(args, "orbit_elevation_deg", Some(20.0), false)?;
        let dist = cam_num(args, "orbit_distance_m", None, true)?;
        if dist.is_some_and(|d| d <= 0.0) {
            return Err(ToolFail::tool("`orbit_distance_m` must be greater than zero"));
        }
        // Elevation is measured off the WORLD horizontal and the ring is built on the
        // vehicle's FLATTENED heading, so "30 degrees up at 10 metres" means the same
        // thing on a machine parked nose-down a bank as on one standing level.
        let veh = cam_veh_expr(args)?;
        tgt_default = " local __t=vec3(__vp.x,__vp.y,__vp.z)";
        format!(
            "{veh} if not __v then error('no such vehicle to orbit',0) end \
             local __vp=__v:getPosition() local __vf=__v:getDirectionVector() \
             local __h=vec3(__vf.x,__vf.y,0) \
             if __h:length()<1e-6 then __h=vec3(0,1,0) end __h:normalize() \
             local __rt=vec3(__h.y,-__h.x,0) \
             local __az=math.rad({}) local __el=math.rad({}) local __d={} \
             local __ch=math.cos(__el)*__d \
             local __p=vec3(__vp.x,__vp.y,__vp.z)+__h*(math.cos(__az)*__ch)\
             +__rt*(math.sin(__az)*__ch)+vec3(0,0,math.sin(__el)*__d)",
            lua_num(az)?,
            lua_num(el)?,
            lua_num(dist)?
        )
    } else {
        return Err(ToolFail::tool(
            "`place` needs a position: absolute (x/y/z), relative \
             (rel_fwd_m/rel_right_m/rel_up_m) or orbit (orbit_distance_m + orbit_*)",
        ));
    };

    let mut tgt_lua = tgt_default.to_string();
    let look = aget_val(args, "look_at")?;
    if !look.is_null() {
        if let PyJson::List(items) = &look {
            if items.len() != 3 {
                return Err(ToolFail::tool("`look_at` as a point must be [x, y, z]"));
            }
            let mut pt = Vec::with_capacity(3);
            for v in items {
                let one = pyjson::dict(vec![("v", v.clone())]);
                pt.push(lua_num(cam_num(&one, "v", None, true)?)?);
            }
            tgt_lua = format!(" local __t=vec3({},{},{})", pt[0], pt[1], pt[2]);
        } else {
            let lid = match look.py_int() {
                Ok(n) => n,
                Err(PyErr::TypeError(_) | PyErr::ValueError(_)) => {
                    return Err(ToolFail::tool(format!(
                        "`look_at` must be [x, y, z] or a vehicle id, got {}",
                        look.py_repr()
                    )));
                }
                Err(e) => return Err(e.into()),
            };
            tgt_lua = format!(
                " local __lv=be:getObjectByID({lid}) \
                 if not __lv then error('no such look_at vehicle',0) end \
                 local __lp=__lv:getPosition() local __t=vec3(__lp.x,__lp.y,__lp.z)"
            );
        }
    }

    let yaw = cam_num(args, "yaw_deg", Some(0.0), false)?;
    let pitch = cam_num(args, "pitch_deg", Some(0.0), false)?;
    let roll = cam_num(args, "roll_deg", Some(0.0), false)?;

    // A resolved look-at target overrides the explicit angles. Both yaw and pitch are
    // derived here in the MOD's convention and converted once, at the call itself.
    let aim = format!(
        " local __y,__pi={},{} if __t then local __d=__t-__p local __l=__d:length() \
         if __l>1e-6 then __y=n360(math.deg(math.atan2(-__d.x,-__d.y))) \
         __pi=math.deg(math.asin(__d.z/__l)) end end",
        lua_num(yaw)?,
        lua_num(pitch)?
    );

    Ok(format!(
        "{CAM_ENSURE_FREE}{N360_LUA}{pos_lua}{tgt_lua}{aim} core_camera.setPosition(0,__p) \
         core_camera.setFreeCameraYawPitchRollDeg(n360(__y-{}),-__pi,{}) return 'placed'",
        py_float_str(CAM_YAW_OFFSET_DEG),
        lua_num(roll)?
    ))
}

/// `_t_camera_control(args)`.
pub(super) fn t_camera_control(args: &PyJson) -> TResult<PyJson> {
    // Argument validation comes BEFORE the world check: a typo'd action is a caller error
    // and must name the valid ones whatever the game is doing. Told "the game may be
    // closed" instead, the agent goes and debugs the game.
    let action = strip_or(&aget_val(args, "action")?, "get")?;
    if !CAMERA_ACTIONS.contains(&action.as_str()) {
        return Err(ToolFail::tool(format!(
            "unknown action {} ({})",
            pyjson::py_str_repr(&action),
            CAMERA_ACTIONS.join(" | ")
        )));
    }
    require_world()?;

    let settle = aget_or(args, "settle_ms", PyJson::Int(CAM_SETTLE_MS_DEFAULT))?;

    let chunk = match action.as_str() {
        "list_modes" => {
            let res = cam_run(CAM_LIST_LUA)?;
            return set_action(result_json(&res, "list_modes")?, "list_modes");
        }
        "get" => return set_action(cam_read(&PyJson::Null)?, "get"),
        "set_mode" => {
            let mode = strip_or(&aget_val(args, "mode")?, "")?;
            if mode.is_empty() {
                return Err(ToolFail::tool(
                    "`mode` is required for set_mode; call list_modes for the names",
                ));
            }
            // Validated rather than escaped: a camera name is an identifier, and refusing
            // anything else keeps quotes out of the composed chunk entirely.
            if !mode
                .chars()
                .all(|c| c.is_alphanumeric() || "._-".contains(c))
            {
                return Err(ToolFail::tool(format!(
                    "`mode` must be a camera name (letters, digits, . _ -), got {}",
                    pyjson::py_str_repr(&mode)
                )));
            }
            format!(
                "local n='{mode}' if n=='free' then {CAM_ENSURE_FREE}else core_camera.setByName(0,n) end return 'mode set'"
            )
        }
        "cycle" => {
            let offset = match cam_num(args, "offset", Some(1.0), false)? {
                Some(f) => pyjson::float_to_int(f)?,
                None => {
                    return Err(PyErr::TypeError(
                        "int() argument must be a string, a bytes-like object or a real number, not 'NoneType'"
                            .to_string(),
                    )
                    .into());
                }
            };
            if offset == 0 {
                return Err(ToolFail::tool(
                    "`offset` must be non-zero (+1 next camera, -1 previous)",
                ));
            }
            format!("core_camera.setVehicleCameraByIndexOffset(0,{offset}) return 'cycled'")
        }
        "reset" => "core_camera.resetCamera(0) return 'reset'".to_string(),
        _ => cam_place_chunk(args)?,
    };

    let len = chunk.chars().count();
    if len > MAX_DATAGRAM_CONTENT {
        return Err(ToolFail::tool(format!(
            "composed camera chunk is {len} bytes, over the {MAX_DATAGRAM_CONTENT}-byte datagram limit"
        )));
    }
    cam_run(&chunk)?;
    // Every action answers with the readback, so a set is self-verifying in one round
    // trip -- and `place` in particular can only work from the free camera, since
    // setPosition and setFreeCameraYawPitchRollDeg both early-return otherwise
    // (core/camera.lua:1088). `is_free` here is therefore proof that it landed rather
    // than an inference from the absence of an error.
    set_action(cam_read(&settle)?, &action)
}

/// `SCREENSHOT_DIR_DEFAULT`: `%LOCALAPPDATA%/beamtel/screenshots`.
pub fn screenshot_dir_default() -> PathBuf {
    beam_base::paths::config_dir().join("screenshots")
}

/// `os.path.expanduser` (ntpath) for a leading `~` alone or `~\` / `~/`.
fn expanduser(p: &str) -> String {
    let Some(rest) = p.strip_prefix('~') else {
        return p.to_string();
    };
    if !(rest.is_empty() || rest.starts_with(['/', '\\'])) {
        return p.to_string();
    }
    let home = match std::env::var_os("USERPROFILE") {
        Some(h) => h.to_string_lossy().into_owned(),
        None => match std::env::var_os("HOMEPATH") {
            Some(hp) => format!(
                "{}{}",
                std::env::var_os("HOMEDRIVE")
                    .unwrap_or_default()
                    .to_string_lossy(),
                hp.to_string_lossy()
            ),
            None => return p.to_string(),
        },
    };
    format!("{home}{rest}")
}

/// `os.path.abspath` (ntpath): absolute and normalized, separators as backslashes.
fn abspath(p: &str) -> PathBuf {
    let p = p.replace('/', "\\");
    std::path::absolute(Path::new(&p)).unwrap_or_else(|_| PathBuf::from(p))
}

/// `os.path.basename(filename) != filename` (ntpath): a drive or any separator.
fn is_bare_name(name: &str) -> bool {
    let b = name.as_bytes();
    let has_drive = b.len() >= 2 && b[1] == b':';
    !has_drive && !name.contains(['/', '\\'])
}

/// The Python exception type an `OSError` from `os.makedirs` / `open` surfaces as.
fn os_error_type(e: &std::io::Error) -> &'static str {
    use std::io::ErrorKind as K;
    match e.kind() {
        K::NotFound => "FileNotFoundError",
        K::PermissionDenied => "PermissionError",
        K::AlreadyExists => "FileExistsError",
        K::NotADirectory => "NotADirectoryError",
        K::IsADirectory => "IsADirectoryError",
        _ => "OSError",
    }
}

fn write_png(directory: &Path, path: &Path, png: &[u8]) -> Result<(), String> {
    use crate::updater::pyos;
    std::fs::create_dir_all(directory)
        .map_err(|e| format!("{}: {}", os_error_type(&e), pyos::win(&e, Some(directory), None)))?;
    std::fs::write(path, png)
        .map_err(|e| format!("{}: {}", os_error_type(&e), pyos::errno(&e, Some(path))))
}

/// `_t_screenshot(args)`: write a PNG of the game to disk. Deliberately does NOT send it
/// anywhere.
pub(super) fn t_screenshot(args: &PyJson) -> TResult<PyJson> {
    let Some(capture) = DEPS.read().as_ref().map(|d| d.capture_png_fn.clone()) else {
        return Err(ToolFail::tool(
            "this beamtel build injected no capture function -- screenshot is unavailable",
        ));
    };

    let hide_ui = aget_or(args, "hide_ui", PyJson::Bool(true))?.truthy();
    // Default TRUE, unlike the AI Describer, because the situations differ: that one fires
    // from a keypress while the user is playing, so the game is already in front. An agent
    // shoots while the user is looking at a terminal, and BeamNG is then usually
    // MINIMIZED -- it parks off-screen and renames itself "- background", so a plain
    // monitor grab returns the desktop. That is a silent, entirely plausible wrong answer,
    // which is the failure this whole server exists to catch.
    let focus_game = aget_or(args, "focus_game", PyJson::Bool(true))?.truthy();
    let settle_ms = aget_val(args, "settle_ms")?;
    let settle_s = if settle_ms.is_null() {
        crate::capture::UI_HIDE_SETTLE_S
    } else {
        py_max(0.0, py_min(settle_ms.py_float()?, SHOT_SETTLE_MS_MAX)) / 1000.0
    };

    let dir_arg = aget_val(args, "dir")?;
    let dir_text = if dir_arg.truthy() {
        dir_arg.py_str()
    } else {
        screenshot_dir_default().to_string_lossy().into_owned()
    };
    let directory = abspath(&expanduser(&dir_text));

    let filename_arg = aget_val(args, "filename")?;
    let filename = if filename_arg.truthy() {
        let mut name = filename_arg.py_str();
        if !is_bare_name(&name) || name == "." || name == ".." {
            return Err(ToolFail::tool(format!(
                "`filename` must be a bare file name, not a path: {}",
                pyjson::py_str_repr(&name)
            )));
        }
        if !name.to_lowercase().ends_with(".png") {
            name.push_str(".png");
        }
        name
    } else {
        // Timestamped to the millisecond: a before/after pair is the main thing this tool
        // gets used for, and two shots in the same second must not collide.
        let now = DateTime::local_now();
        format!(
            "{}_{:03}.png",
            now.strftime("beam_%Y%m%d_%H%M%S"),
            now.micro / 1000
        )
    };

    let path = directory.join(&filename);

    // The Rust capture error carries the exception's message without its type name.
    let got = catch_unwind(AssertUnwindSafe(|| capture(hide_ui, settle_s, focus_game)));
    let (png, meta) = match got {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => return Err(ToolFail::tool(format!("screen capture failed: {e}"))),
        Err(_) => {
            return Err(ToolFail::tool(
                "screen capture failed: RuntimeError: capture panicked",
            ));
        }
    };
    if png.is_empty() {
        return Err(ToolFail::tool("screen capture returned no data"));
    }

    if let Err(e) = write_png(&directory, &path, &png) {
        return Err(ToolFail::tool(format!(
            "could not write {}: {e}",
            path.to_string_lossy()
        )));
    }

    let mut out = pyjson::dict_of(vec![
        ("path", PyJson::Str(path.to_string_lossy().into_owned())),
        ("bytes", PyJson::Int(png.len() as i64)),
        ("hide_ui", PyJson::Bool(hide_ui)),
        (
            "note",
            PyJson::from("written to disk only -- not sent to any model. Read the file to view it."),
        ),
    ]);
    // What was actually captured rides back with it. A grab of the desktop because the
    // game would not come up looks exactly like a real screenshot to everything except
    // this field.
    if let PyJson::Dict(m) = json_to_pyjson(&meta) {
        for (k, v) in m {
            out.insert(k, v);
        }
    }
    if focus_game && !out.get("game_window_found").is_some_and(PyJson::truthy) {
        out.insert(
            "warning".into(),
            PyJson::from("no BeamNG window found -- this is a grab of the primary monitor"),
        );
    }
    Ok(PyJson::Dict(out))
}

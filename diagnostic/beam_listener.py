# diagnostic_listener.py
import socket
import struct
from datetime import datetime
import os

# --- Configuration ---
UDP_IP = "0.0.0.0"
UDP_PORT = 4444
LOG_FILE = "diagnostic_log_expanded.log"

# --- MODIFICATION: Updated to the final, fully expanded protocol format ---
# This struct format now includes all 29 floats and matches the latest Lua file.
# The total size is 132 bytes.
EXT_FORMAT = '<H4sBx9fII20f'
EXT_SIZE = struct.calcsize(EXT_FORMAT)

def main():
    """A simple listener to diagnose the full, expanded telemetry packet."""
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        sock.bind((UDP_IP, UDP_PORT))
    except OSError as e:
        print(f"Error binding to {UDP_IP}:{UDP_PORT} - {e}")
        return

    print("--- BeamNG Expanded Telemetry Diagnostic Listener ---")
    print(f"Listening for UDP packets on port {UDP_PORT}...")
    print(f"Expecting packet size: {EXT_SIZE} bytes.")
    print(f"Logging all raw values to: {os.path.abspath(LOG_FILE)}")
    print("Press Ctrl+C to exit.")

    try:
        while True:
            data, addr = sock.recvfrom(2048)

            if len(data) != EXT_SIZE:
                # This message is important for debugging size mismatches
                print(f"Ignoring packet of wrong size. Got: {len(data)}, Expected: {EXT_SIZE}")
                continue

            try:
                # Unpack the full data structure
                unpacked_data = struct.unpack(EXT_FORMAT, data)

                # Assign all values for clarity
                gear_bytes = unpacked_data[1]
                speed, rpm, rpm_max, turbo, turbo_max, eng_temp, fuel, oil_pressure, oil_temp = unpacked_data[3:12]
                
                (
                    throttle, brake, clutch,
                    air_pressure, air_pressure_max,
                    clutch_temp, g_lat, g_lon,
                    tire_p_fl, tire_p_fr, tire_p_rl, tire_p_rr,
                    tire_t_fl, tire_t_fr, tire_t_rl, tire_t_rr,
                    brake_t_fl, brake_t_fr, brake_t_rl, brake_t_rr
                ) = unpacked_data[14:]
                
                gear = gear_bytes.decode('utf-8', errors='ignore').strip('\x00')
                timestamp = datetime.now().strftime('%Y-%m-%d %H:%M:%S.%f')[:-3]

                # Build a comprehensive output string for all new data
                output_lines = [
                    f"--- Packet at {timestamp} | Gear: {gear} ---",
                    f"Speed: {speed*2.23694:.1f} MPH | RPM: {rpm:.0f}",
                    "--- Core Temps & Pressures ---",
                    f"  Engine Temp: {eng_temp:.1f} C | Oil Temp: {oil_temp:.1f} C | Clutch Temp: {clutch_temp:.1f} C",
                    f"  Oil Pressure: {oil_pressure*14.504:.1f} PSI | Turbo: {turbo*14.504:.1f} PSI",
                    f"  Air Pressure: {air_pressure:.1f} PSI | Air Pressure Max: {air_pressure_max:.1f} PSI",
                    "--- Tires ---",
                    f"  Pressure (PSI): FL={tire_p_fl:.1f}, FR={tire_p_fr:.1f}, RL={tire_p_rl:.1f}, RR={tire_p_rr:.1f}",
                    f"  Temp (C):       FL={tire_t_fl:.1f}, FR={tire_t_fr:.1f}, RL={tire_t_rl:.1f}, RR={tire_t_rr:.1f}",
                    "--- Brakes & G-Force ---",
                    f"  Brake Temp (C): FL={brake_t_fl:.1f}, FR={brake_t_fr:.1f}, RL={brake_t_rl:.1f}, RR={brake_t_rr:.1f}",
                    f"  G-Force Lat: {g_lat:.2f} | G-Force Lon: {g_lon:.2f}"
                ]
                output_string = "\n".join(output_lines)

                print(output_string)
                print()
                
                with open(LOG_FILE, 'a') as f:
                    f.write(output_string + "\n\n")

            except struct.error as e:
                print(f"*** ERROR UNPACKING DATA: {e}. This indicates a severe mismatch between Lua and Python structs. ***")
            except Exception as e:
                print(f"An unexpected error occurred: {e}")

    except KeyboardInterrupt:
        print("\nListener stopped by user.")
    finally:
        sock.close()
        print("Socket closed.")

if __name__ == "__main__":
    main()
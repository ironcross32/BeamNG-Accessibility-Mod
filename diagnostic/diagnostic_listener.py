import socket
import struct
import time

def start_telemetry_listener(ip="127.0.0.1", port=4444):
    # CORRECTED STRUCT FORMAT (136 Bytes total):
    # < = Little Endian
    # H = flags (2), 4s = gear (4), b = plid (1), x = padding (1) -> 8 bytes
    # 9f = basic floats (36)
    # II = lights (8)
    # 21f = Inputs/Pneumatics/G-Force/Tires (84) 
    # (3 inputs + 1 steering + 2 air + 3 exp + 12 tires = 21)
    fmt = "<H4sbx9fII22f"
    expected_size = struct.calcsize(fmt)
    
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    # Allow multiple apps to listen if the OS supports it
    sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    sock.bind((ip, port))
    
    last_print_time = 0
    print(f"--- Diagnostic Listener Started ---")
    print(f"Targeting Packets: {expected_size} bytes")
    print(f"Listening on {ip}:{port}...\n")

    try:
        while True:
            data, addr = sock.recvfrom(1024)
            data_len = len(data)

            # 1. FILTERING & DIAGNOSTICS
            if data_len != expected_size:
                # Optional: Uncomment the line below to see MotionSim packet sizes
                # print(f"Ignored MotionSim packet: {data_len} bytes")
                continue 

            # 2. RATE LIMITING (1 Packet Per Second)
            current_time = time.time()
            if current_time - last_print_time >= 1.0:
                try:
                    unpacked = struct.unpack(fmt, data)
                    
                    # EXTRACTING DATA
                    # The index for steering is 17:
                    # [0]flags, [1]gear, [2]plid, [3-11]basic, [12-13]lights, 
                    # [14]throttle, [15]brake, [16]clutch, [17]steering
                    gear = unpacked[1].decode('utf-8').strip('\x00')
                    speed_kph = unpacked[3] * 3.6
                    steering = unpacked[17]
                    actual_steering = unpacked[18]
                    
                    print(f"[{time.strftime('%H:%M:%S')}] Size: {data_len} | Gear: {gear} | Speed: {speed_kph:3.0f} | Steering: {steering:5.2f} | Steering Angle: {actual_steering:5.2f}")
                    
                    last_print_time = current_time
                except Exception as e:
                    print(f"Unpack error: {e}")

    except KeyboardInterrupt:
        print("\nStopping listener...")
    finally:
        sock.close()

if __name__ == "__main__":
    start_telemetry_listener()
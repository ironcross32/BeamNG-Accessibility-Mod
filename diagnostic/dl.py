import socket
import struct
import time

# --- Configuration ---
UDP_IP = "0.0.0.0"
UDP_PORT = 4444

# --- New Diagnostic Packet Structure ---
# This matches the new Lua script, containing 22 floats.
DIAG_FORMAT = '<22f'
DIAG_SIZE = struct.calcsize(DIAG_FORMAT) # Should be 88 bytes

def main():
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        sock.bind((UDP_IP, UDP_PORT))
    except OSError as e:
        print(f"Error binding to {UDP_IP}:{UDP_PORT} - {e}")
        return

    print("--- BeamNG Super Diagnostic Listener ---")
    print(f"Listening on port {UDP_PORT} for diagnostic packets ({DIAG_SIZE} bytes).")
    print("Drive, brake, and turn to see if 'Direct Query' values change from -1.0")
    print("-" * 50)

    last_print_time = 0
    try:
        while True:
            data, addr = sock.recvfrom(1024)

            if len(data) != DIAG_SIZE:
                continue

            try:
                unpacked = struct.unpack(DIAG_FORMAT, data)
                
                # Throttle printing to once per second to make it readable
                current_time = time.time()
                if current_time - last_print_time < 1.0:
                    continue
                last_print_time = current_time

                # Clear console for cleaner output
                print("\033[H\033[J", end="")
                
                print(f"--- Telemetry Snapshot at {time.strftime('%H:%M:%S')} ---")
                
                print("\n--- G-Forces ---")
                print(f"  Source      | Lateral | Longitudinal")
                print(f"  Electrics   | {unpacked[0]:<7.2f} | {unpacked[1]:<7.2f}")
                print(f"  Sensors     | {unpacked[2]:<7.2f} | {unpacked[3]:<7.2f}  <-- Should be this one")

                print("\n--- Clutch Temperature (C) ---")
                print(f"  Electrics: {unpacked[4]:.1f}")
                print(f"  Direct Query: {unpacked[5]:.1f}  <-- Should be this one")
                
                print("\n--- Brake Temperatures (C) ---")
                print(f"  Source      |   FL   |   FR   |   RL   |   RR   |")
                print(f"  Electrics   | {unpacked[6]:<6.1f} | {unpacked[7]:<6.1f} | {unpacked[8]:<6.1f} | {unpacked[9]:<6.1f} |")
                print(f"  Direct Query| {unpacked[10]:<6.1f} | {unpacked[11]:<6.1f} | {unpacked[12]:<6.1f} | {unpacked[13]:<6.1f} | <-- Should be this one")

                print("\n--- Tire Temperatures (C) ---")
                print(f"  Source      |   FL   |   FR   |   RL   |   RR   |")
                print(f"  Electrics   | {unpacked[14]:<6.1f} | {unpacked[15]:<6.1f} | {unpacked[16]:<6.1f} | {unpacked[17]:<6.1f} |")
                print(f"  Direct Query| {unpacked[18]:<6.1f} | {unpacked[19]:<6.1f} | {unpacked[20]:<6.1f} | {unpacked[21]:<6.1f} | <-- Should be this one")


            except struct.error as e:
                print(f"Error unpacking data: {e}")

    except KeyboardInterrupt:
        print("\nListener stopped.")
    finally:
        sock.close()

if __name__ == "__main__":
    main()
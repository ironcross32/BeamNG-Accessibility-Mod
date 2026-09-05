def list_audio_devices():
    """
    Prints a formatted list of all available audio devices, including their
    API, and marks the default devices.
    """
    import sounddevice as sd
    try:
        # Get host API and device information
        host_apis = sd.query_hostapis()
        all_devices = sd.query_devices()
        
        # Get default device indices
        default_input_idx = sd.default.device[0]
        default_output_idx = sd.default.device[1]

        print("--- Readable Audio Device List ---")
        for i, device in enumerate(all_devices):
            device_name = device['name']
            host_api_index = device['hostapi']
            host_api_name = host_apis[host_api_index]['name']
            
            # Check for default status
            is_default_in = " (Default Input)" if i == default_input_idx else ""
            is_default_out = " (Default Output)" if i == default_output_idx else ""
            
            print(
                f"Device #{i}: \"{device_name}\"{is_default_in}{is_default_out}\n"
                f"  Host API:      {host_api_name} (index {host_api_index})\n"
                f"  Max Inputs:    {device['max_input_channels']}\n"
                f"  Max Outputs:   {device['max_output_channels']}\n"
                f"  Default Rate:  {device['default_samplerate']} Hz\n"
            )
        print("-" * 34)

    except Exception as e:
        print(f"An error occurred: {e}")
if __name__ == "__main__":
    list_audio_devices()
import shutil
import os

def check_disk_space():
    path = "/mnt/nvme0n1p2"
    threshold_gb = 50
    
    # Get disk usage statistics
    usage = shutil.disk_usage(path)
    available_gb = usage.free / (1024**3)
    
    print(f"Available space on {path}: {available_gb:.2f} GB")
    
    if available_gb < threshold_gb:
        with open("disk_warning.log", "a") as f:
            f.write(f"Warning: Available disk space on {path} is {available_gb:.2f} GB, which is below the threshold of {threshold_gb} GB.\n")
        print("Warning written to disk_warning.log")
    else:
        print("Disk space is sufficient.")

if __name__ == "__main__":
    check_disk_space()

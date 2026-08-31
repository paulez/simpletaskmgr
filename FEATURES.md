# Simpletaskmgr - Process Manager

## Feature Overview

### Current Features

1. **Process List Display**
   - Shows all running processes with PID and CPU usage
   - Sorted by CPU usage percentage (highest first)
   - Real-time CPU tracking using time delta calculations

2. **User Filtering**
   - Displays only current user's processes
   - Uses `users::get_current_uid()` for accurate user identification

3. **Real-time CPU Monitoring**
   - Tracks CPU usage for each process
   - Calculates percentage based on elapsed time between measurements
   - Updates CPU information every 1 second

4. **Process Detail View**
    - Click on any process to view detailed information (right pane)
    - View process name, PID, UID, username, and CPU usage
    - Detail values update in place on each refresh while the process is selected
    - The detail pane is only visible while a process is selected; when none is selected the process list expands to the full width
    - No process is selected on launch

     5. **CPU Status**
     - Live readout of current CPU frequency (core 0) and CPU temperature in a
       status line below the usage graph
     - Frequency and temperature traced over time on the usage graph, each on
       its own dedicated axis (frequency top-anchored at `scaling_max_freq`,
       temperature 0-100 °C)
     - Sources: `cpufreq/scaling_cur_freq` for frequency; CPU `hwmon` sensor's
       hottest `tempN_input` for temperature; both blank when unavailable
     - Read failures carry the previous value forward so the series doesn't dip
       to zero spuriously

     6. **Process Signal Management**
     - Send SIGHUP and SIGKILL signals to a selected process from its detail view
     - Success/failure reported in a status line below the buttons and in the log
     - Failures surfaced with context (e.g. permission denied, process gone)

     7. **Persistent Settings**
     - Settings stored in `~/.config/simpletaskmgr/settings.toml` (XDG config dir)
     - "Show all processes" filter and refresh interval (0.5s / 1.5s / 3s) persist across restarts
     - Change them via the Settings popover in the toolbar; "Reset to defaults"
     - Corrupt or partial config files fall back to defaults instead of failing to start

### Planned Features

1. **Enhanced User Experience**
    - Improved visual layout and styling
    - Better error handling and user feedback

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

5. **Process Signal Management**
   - Send SIGHUP and SIGKILL signals to a selected process from its detail view
   - Success/failure reported in a status line below the buttons and in the log
   - Failures surfaced with context (e.g. permission denied, process gone)

### Planned Features

1. **Enhanced User Experience**
   - Improved visual layout and styling
   - Better error handling and user feedback
   - Persistent user preferences for display settings

2. **Show all users toggle**
   - The settings toggle to switch between current user and all users is documented
     but filtering still only shows the current user's processes
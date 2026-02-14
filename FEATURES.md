# Simpletaskmgr - Process Manager

## Feature Overview

### Current Features

1. **Process List Display**
   - Shows all running processes with PID and CPU usage
   - Sorted by CPU usage percentage (highest first)
   - Real-time CPU tracking using time delta calculations

2. **User Filtering**
   - Displays only current user's processes by default
   - Settings menu toggle to switch between current user and all users
   - Uses `users::get_current_uid()` for accurate user identification

3. **Real-time CPU Monitoring**
   - Tracks CPU usage for each process
   - Calculates percentage based on elapsed time between measurements
   - Updates CPU information every 1 second

### Planned Features

1. **Process Detail View**
   - Double-click on any process to view detailed information
   - View process name, PID, UID, username, and CPU usage
   - Easy navigation back to main process list

2. **Process Signal Management**
   - Send SIGHUP and SIGKILL signals to processes from detailed view
   - Process termination capabilities with user interface
   - Safe and controlled process termination

3. **Enhanced User Experience**
   - Improved visual layout and styling
   - Better error handling and user feedback
   - Persistent user preferences for display settings
# Simple Task Manager

A lightweight, interactive system process manager built with Rust that displays all running processes on your system in a user-friendly graphical interface.

## Features

- **Real-time Process Display**: Shows PID, Real User ID (RUID), and process name for each running process
- **Auto-refresh**: The process list automatically updates every second
- **Efficient Rendering**: Uses virtualized scrolling for smooth performance with many processes
- **Modern UI**: Built with the Floem GUI framework for a responsive, clean interface
- **Linux Native**: Direct access to Linux `/proc` filesystem for accurate process information

## Requirements

- Rust (1.70 or later)
- Linux operating system (uses procfs to read from `/proc`)

## Building

```bash
cargo build --release
```

## Running

```bash
cargo run --release
```

Or run with cargo directly:

```bash
cargo run
```

## Dependencies

The project uses the following Rust crates:

- **floem** 0.2.0 - Modern GUI framework
- **im** 15.1.0 - Immutable data structures
- **procfs** 0.17.0 - Linux procfs filesystem bindings

## How It Works

The application reads process information directly from the Linux `/proc` filesystem using the `procfs` crate. It then displays the data in a virtualized list that automatically refreshes every second using reactive programming with the Floem framework.

Each process entry shows:
- **PID**: Process identifier
- **RUID**: Real user ID (the user who owns the process)
- **Name**: Process name

## License

This project is open source and available under the same license as the Rust toolchain.
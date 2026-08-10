# displayd

A small Rust library with a daemon and CLI for controlling external monitors. VESA DDC/CI support is required.
`displayd` is primarily designed to make monitor brightness control feel fast and responsive. Existing tools such as `ddcutil` can introduce enough latency for interactive brightness changes to feel sluggish. `displayd` aims to solve this by providing a persistent, low-latency service for communicating with monitors, while also establishing a system that is not limited to CLI-based control. Its architecture is intended to be embeddable in other applications as well, while exposing monitor state changes, particularly brightness changes, as events.

## Installation

Before installing `displayd`, make sure you have completed the [I²C device permissions](#i2c-device-permissions) setup described in the Requirements section.

`displayd` consists of two binaries:

* `displayd` — the daemon that communicates with the external monitor.
* `displayctl` — the CLI used to control the daemon.

The daemon is managed as a **systemd user service**. Alternatively, you can launch `displayd` yourself through another startup mechanism.

All release artifacts are available in the [GitHub Releases](https://github.com/That-Developer-Guy/displayd/releases):

* `displayd`
* `displayctl`
* `displayd.service`

### 1. Download the release files

Download `displayd`, `displayctl`, and `displayd.service` from the desired release.

For example, using `wget`:

```bash
wget https://github.com/That-Developer-Guy/displayd/releases/latest/download/displayd
wget https://github.com/That-Developer-Guy/displayd/releases/latest/download/displayctl
wget https://github.com/That-Developer-Guy/displayd/releases/latest/download/displayd.service
```

### 2. Install the binaries

Create the local binary directory if it doesn't already exist:

```bash
mkdir -p ~/.local/bin
```

Copy the binaries into it and make them executable:

```bash
install -m 755 displayd ~/.local/bin/displayd
install -m 755 displayctl ~/.local/bin/displayctl
```

### 3. Add `~/.local/bin` to your `PATH`

If `~/.local/bin` is not already in your `PATH`, add it to your shell configuration.

For **bash**:

```bash
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bashrc
source ~/.bashrc
```

For **zsh**:

```bash
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.zshrc
source ~/.zshrc
```

You can verify that it worked with:

```bash
which displayctl
which displayd
```

Both commands should point to `~/.local/bin/`.

### 4. Install the systemd service

Create the systemd user service directory if it doesn't already exist:

```bash
mkdir -p ~/.config/systemd/user
```

Copy the service file into it:

```bash
cp displayd.service ~/.config/systemd/user/displayd.service
```

Reload the systemd user manager so it recognizes the new service:

```bash
systemctl --user daemon-reload
```

### 5. Start the daemon

Start `displayd` with:

```bash
systemctl --user start displayd.service
```

You can check its status with:

```bash
systemctl --user status displayd.service
```

To start `displayd` automatically when you log in, enable the service:

```bash
systemctl --user enable displayd.service
```

Or enable and start it in one command:

```bash
systemctl --user enable --now displayd.service
```

### Troubleshooting

If the daemon does not start, view its logs with:

```bash
journalctl --user -u displayd.service
```

To follow the logs in real time:

```bash
journalctl --user -u displayd.service -f
```

Once the daemon is running, you can use `displayctl` to control your monitor.

## Build from source

Building `displayd` from source requires the **Rust toolchain** and git.

### 1. Clone the repository

```bash
git clone https://github.com/That-Developer-Guy/displayd.git
cd displayd
```

### 2. Build the release binaries

Build both binaries with cargo:

```bash
cargo build --release
```

The resulting binaries will be located at:

```text
target/release/displayd
target/release/displayctl
```

The systemd service file is included in the repository root:

```text
displayd.service
```

You can then follow the [installation instructions](#installation) above, using these locally built files instead of downloading the release files.

For example:

```bash
install -m 755 target/release/displayd ~/.local/bin/displayd
install -m 755 target/release/displayctl ~/.local/bin/displayctl

mkdir -p ~/.config/systemd/user
cp displayd.service ~/.config/systemd/user/displayd.service

systemctl --user daemon-reload
systemctl --user enable --now displayd.service
```

## Usage

```text
Control DDC/CI displays

Usage: displayctl [OPTIONS] <COMMAND>

Commands:
  brightness  Show and modify the brightness level
  contrast    Show and modify the contrast level
  dim         Apply dim (10% of brightness level)
  undim       Reset dim (10% of brightness level)
  watch       Listen for display changes
  list        List available monitors
  help        Print this message or the help of the given subcommand(s)

Options:
  -m, --monitor <MONITOR>  Monitor connector (for example DP-2)
  -v, --verbose            Show detailed output
      --json               Output JSON
  -h, --help               Print help
```

Alternatively, run `displayctl --help` for more information.

## Contributions and Issues

Contributions, bug reports, and feature requests are always welcome!

If you encounter an issue, it would be very helpful to provide as much relevant information as possible. In particular, a short description of the problem and the versions of your operating system, Rust toolchain, and `displayd` can be very useful when diagnosing issues.

For hardware-related issues, please also include your monitor model and any relevant error messages or logs, for example from the kernel.

I will do my best to help troubleshoot issues, but it is difficult to test every monitor myself. `displayd` currently requires support for **VESA DDC/CI**, and monitors that do not meet this requirement will not work. Even among DDC/CI-compatible monitors, there can be differences in how it is implemented, which can only really be solved if you have access to the hardware.

## Requirements

* Linux
* A monitor supporting **VESA DDC/CI**
* `systemd` with user-service support
* Read/write access to the relevant `/dev/i2c-*` devices
* Rust toolchain (only required when building from source)
* Git (only required when building from source)

> **Important:** `displayd` communicates with monitors through Linux I²C devices (`/dev/i2c-*`). Your user must have read/write access to the relevant devices. See [I²C device permissions](#i2c-device-permissions) before continuing with the installation.

<a id="i2c-device-permissions"></a>
### I²C device permissions

First, check the permissions of your I²C devices:

```bash
ls -l /dev/i2c-*
```

For example, you might see:

```text
crw-rw---- 1 root i2c ... /dev/i2c-1
```

In this case, the devices are owned by the `i2c` group. Add your user to that group:

```bash
sudo usermod -aG i2c "$USER"
```

You must then **log out and log back in** for the new group membership to take effect.

Verify that you are a member of the group:

```bash
groups
```

You should see `i2c` in the list.

If your I²C devices belong to a different group, add your user to that group instead.

> **Note:** The group and permissions assigned to `/dev/i2c-*` can vary between Linux distributions. Do not assume that the group is always `i2c`; use `ls -l /dev/i2c-*` to determine the appropriate group on your system.
#!/bin/bash
export CARGO_HOME=/usr/local/cargo
export RUSTUP_HOME=/usr/local/rustup
export CARGO_PKG_NAME=duet-sandbox
export PATH=/usr/local/cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
mount -t proc proc /proc 2>/dev/null || true
mount -t sysfs sysfs /sys 2>/dev/null || true
mount -t devtmpfs devtmpfs /dev 2>/dev/null || true
mkdir -p /dev/pts /tmp /run
mount -t devpts devpts /dev/pts 2>/dev/null || true
chmod 1777 /tmp
/duet-up-lo
export DUET_SANDBOX_BRIDGE=/cache/build/debug/duet-sandbox-bridge
export HOME=/root
export RUST_TEST_THREADS=1
printf '\nDUET_FULL_SYSTEM_START\n'
uname -a
cat /boot/config-* | grep -E '^CONFIG_(SECCOMP|SECCOMP_FILTER|USER_NS|NET_NS|PID_NS)='
status=0
/cache/build/debug/duet --version || status=1
/cache/build/debug/deps/duet_sandbox-4dc1122b89fbc72b --nocapture || status=1
/cache/build/debug/deps/network-31ca8a7cf091999c --nocapture || status=1
printf '\nDUET_FULL_SYSTEM_STATUS=%s\n' "$status"
sync
exit "$status"

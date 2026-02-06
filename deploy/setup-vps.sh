#!/usr/bin/env bash
# deploy/setup-vps.sh — One-time VPS provisioning for Hetzner CX32
#
# Run as root on a fresh Ubuntu 24.04 VPS:
#   scp deploy/setup-vps.sh root@vps:/tmp/
#   ssh root@vps 'bash /tmp/setup-vps.sh'

set -euo pipefail

echo "==> Updating system..."
apt-get update && apt-get upgrade -y

echo "==> Installing Podman..."
apt-get install -y podman podman-compose

echo "==> Configuring firewall (SSH only)..."
apt-get install -y ufw
ufw default deny incoming
ufw default allow outgoing
ufw allow ssh
ufw --force enable

echo "==> Creating deploy user..."
if ! id deploy &>/dev/null; then
    useradd -m -s /bin/bash deploy
    usermod -aG sudo deploy
    # Copy root SSH keys to deploy user
    mkdir -p /home/deploy/.ssh
    cp /root/.ssh/authorized_keys /home/deploy/.ssh/ 2>/dev/null || true
    chown -R deploy:deploy /home/deploy/.ssh
    chmod 700 /home/deploy/.ssh
    chmod 600 /home/deploy/.ssh/authorized_keys 2>/dev/null || true
    echo "  Created user 'deploy' with sudo access"
else
    echo "  User 'deploy' already exists"
fi

echo "==> Creating data directories..."
mkdir -p /data/parquet /data/ipc /data/catalog /data/backups
chown -R deploy:deploy /data

echo "==> Configuring journald log rotation (max 500MB)..."
mkdir -p /etc/systemd/journald.conf.d
cat > /etc/systemd/journald.conf.d/size-limit.conf <<'EOF'
[Journal]
SystemMaxUse=500M
RuntimeMaxUse=200M
EOF
systemctl restart systemd-journald

echo "==> Hardening SSH..."
# Disable password auth (key-only)
if ! grep -q "^PasswordAuthentication no" /etc/ssh/sshd_config; then
    sed -i 's/^#*PasswordAuthentication.*/PasswordAuthentication no/' /etc/ssh/sshd_config
    sed -i 's/^#*PermitRootLogin.*/PermitRootLogin prohibit-password/' /etc/ssh/sshd_config
    systemctl restart sshd
    echo "  SSH hardened: password auth disabled, root login restricted"
fi

echo "==> Setting up backup cron..."
CRON_LINE="5 0 * * * /home/deploy/barter/backup.sh >> /data/backups/backup.log 2>&1"
(crontab -u deploy -l 2>/dev/null | grep -v backup.sh; echo "$CRON_LINE") | crontab -u deploy -
echo "  Backup cron set: daily at 00:05 UTC"

echo "==> Setting up disk monitor cron..."
DISK_CRON="*/30 * * * * /home/deploy/barter/status.sh --check-disk >> /data/backups/disk-monitor.log 2>&1"
(crontab -u deploy -l 2>/dev/null | grep -v status.sh; echo "$DISK_CRON") | crontab -u deploy -
echo "  Disk monitor cron set: every 30 minutes"

echo ""
echo "==> VPS setup complete!"
echo "    Data dirs:  /data/{parquet,ipc,catalog,backups}"
echo "    Deploy dir: /home/deploy/barter/ (created on first deploy)"
echo "    Firewall:   SSH only (port 22)"
echo "    Next step:  Run ./deploy/deploy.sh from your Mac"

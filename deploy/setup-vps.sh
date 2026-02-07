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

echo "==> Creating deployer user..."
if ! id deployer &>/dev/null; then
    useradd -m -s /bin/bash deployer
    usermod -aG sudo deployer
    mkdir -p /home/deployer/.ssh
    cp /root/.ssh/authorized_keys /home/deployer/.ssh/ 2>/dev/null || true
    chown -R deployer:deployer /home/deployer/.ssh
    chmod 700 /home/deployer/.ssh
    chmod 600 /home/deployer/.ssh/authorized_keys 2>/dev/null || true
    echo "  Created user 'deployer' with sudo access"
else
    echo "  User 'deployer' already exists"
fi

echo "==> Creating data directories..."
mkdir -p /data/parquet /data/ipc /data/catalog /data/features /data/_checkpoints /data/backups
chown -R deployer:deployer /data

echo "==> Configuring journald log rotation (max 500MB)..."
mkdir -p /etc/systemd/journald.conf.d
cat > /etc/systemd/journald.conf.d/size-limit.conf <<'EOF'
[Journal]
SystemMaxUse=500M
RuntimeMaxUse=200M
EOF
systemctl restart systemd-journald

echo "==> Hardening SSH..."
if ! grep -q "^PasswordAuthentication no" /etc/ssh/sshd_config; then
    sed -i 's/^#*PasswordAuthentication.*/PasswordAuthentication no/' /etc/ssh/sshd_config
    sed -i 's/^#*PermitRootLogin.*/PermitRootLogin prohibit-password/' /etc/ssh/sshd_config
    systemctl restart ssh
    echo "  SSH hardened: password auth disabled, root login restricted"
fi

echo "==> Setting up backup cron..."
CRON_LINE="5 0 * * * /home/deployer/barter/backup.sh >> /data/backups/backup.log 2>&1"
(crontab -u deployer -l 2>/dev/null | grep -v backup.sh; echo "$CRON_LINE") | crontab -u deployer -
echo "  Backup cron set: daily at 00:05 UTC"

echo "==> Setting up disk monitor cron..."
DISK_CRON="*/30 * * * * /home/deployer/barter/status.sh --check-disk >> /data/backups/disk-monitor.log 2>&1"
(crontab -u deployer -l 2>/dev/null | grep -v "check-disk"; echo "$DISK_CRON") | crontab -u deployer -
echo "  Disk monitor cron set: every 30 minutes"

echo "==> Setting up heartbeat watchdog cron..."
HB_CRON="*/5 * * * * /home/deployer/barter/status.sh --check-heartbeat >> /data/backups/heartbeat-monitor.log 2>&1"
(crontab -u deployer -l 2>/dev/null | grep -v "check-heartbeat"; echo "$HB_CRON") | crontab -u deployer -
echo "  Heartbeat watchdog cron set: every 5 minutes"

echo ""
echo "==> VPS setup complete!"
echo "    Data dirs:  /data/{parquet,ipc,catalog,features,_checkpoints,backups}"
echo "    Deploy dir: /home/deployer/barter/ (created on first deploy)"
echo "    Firewall:   SSH only (port 22)"
echo "    Next step:  Run ./deploy/deploy.sh from your Mac"

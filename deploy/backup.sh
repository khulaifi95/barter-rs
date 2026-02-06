#!/usr/bin/env bash
# deploy/backup.sh — Compress old parquet data and rotate local backups
#
# Runs daily via cron at 00:05 UTC:
#   5 0 * * * /home/deployer/barter/backup.sh >> /data/backups/backup.log 2>&1

set -euo pipefail

PARQUET_DIR="/data/parquet"
BACKUP_DIR="/data/backups"
COMPRESS_AFTER_DAYS=7
DELETE_AFTER_DAYS=30

echo "$(date -u '+%Y-%m-%d %H:%M:%S UTC') — Starting backup..."

# ── Compress parquet files older than N days ───────────────────
COMPRESSED=0
while IFS= read -r -d '' dir; do
    DIRNAME="$(basename "$dir")"
    ARCHIVE="$BACKUP_DIR/${DIRNAME}.tar.gz"
    if [ ! -f "$ARCHIVE" ]; then
        tar -czf "$ARCHIVE" -C "$(dirname "$dir")" "$DIRNAME"
        echo "  Compressed: $DIRNAME → $ARCHIVE"
        COMPRESSED=$((COMPRESSED + 1))
    fi
done < <(find "$PARQUET_DIR" -mindepth 1 -maxdepth 1 -type d -mtime +$COMPRESS_AFTER_DAYS -print0 2>/dev/null)
echo "  Compressed $COMPRESSED directories"

# ── Delete local parquet directories older than N days ─────────
DELETED=0
while IFS= read -r -d '' dir; do
    DIRNAME="$(basename "$dir")"
    ARCHIVE="$BACKUP_DIR/${DIRNAME}.tar.gz"
    # Only delete if backup exists
    if [ -f "$ARCHIVE" ]; then
        rm -rf "$dir"
        echo "  Deleted: $dir (backup exists)"
        DELETED=$((DELETED + 1))
    fi
done < <(find "$PARQUET_DIR" -mindepth 1 -maxdepth 1 -type d -mtime +$DELETE_AFTER_DAYS -print0 2>/dev/null)
echo "  Deleted $DELETED old directories"

# ── Delete old backups (90 days) ───────────────────────────────
OLD_BACKUPS=0
while IFS= read -r -d '' archive; do
    rm -f "$archive"
    echo "  Removed old backup: $archive"
    OLD_BACKUPS=$((OLD_BACKUPS + 1))
done < <(find "$BACKUP_DIR" -name "*.tar.gz" -mtime +90 -print0 2>/dev/null)
echo "  Removed $OLD_BACKUPS old backups"

# ── Disk usage summary ────────────────────────────────────────
echo "  Parquet: $(du -sh "$PARQUET_DIR" 2>/dev/null | cut -f1)"
echo "  Backups: $(du -sh "$BACKUP_DIR" 2>/dev/null | cut -f1)"
echo "  Disk:    $(df -h /data | tail -1 | awk '{print $3 " used / " $2 " total (" $5 " used)"}')"
echo "$(date -u '+%Y-%m-%d %H:%M:%S UTC') — Backup complete."

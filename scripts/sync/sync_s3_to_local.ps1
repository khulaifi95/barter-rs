# Sync S3 trading data to local storage (Windows)
#
# Usage:
#   .\sync_s3_to_local.ps1 [-Verify] [-CleanS3]
#
# Environment variables:
#   S3_BUCKET       - S3 bucket name (default: trading-data)
#   LOCAL_DATA_DIR  - Local data directory (default: C:\trading-data\recent)
#   AWS_PROFILE     - AWS CLI profile (optional)
#
# Prerequisites:
#   - AWS CLI configured with proper credentials
#   - Sufficient local disk space

param(
    [switch]$Verify,
    [switch]$CleanS3
)

$ErrorActionPreference = "Stop"

# Configuration
$S3_BUCKET = if ($env:S3_BUCKET) { $env:S3_BUCKET } else { "trading-data" }
$LOCAL_DATA_DIR = if ($env:LOCAL_DATA_DIR) { $env:LOCAL_DATA_DIR } else { "C:\trading-data\recent" }

Write-Host "=============================================="
Write-Host "S3 to Local Data Sync"
Write-Host "=============================================="
Write-Host ""
Write-Host "S3 Bucket:      s3://$S3_BUCKET"
Write-Host "Local Dir:      $LOCAL_DATA_DIR"
Write-Host "AWS Profile:    $($env:AWS_PROFILE ?? 'default')"
Write-Host ""

# Create local directories if they don't exist
$dirs = @(
    "$LOCAL_DATA_DIR\bars_1m",
    "$LOCAL_DATA_DIR\trades",
    "$LOCAL_DATA_DIR\extended_bars"
)

foreach ($dir in $dirs) {
    if (-not (Test-Path $dir)) {
        New-Item -ItemType Directory -Path $dir -Force | Out-Null
    }
}

# Sync bars
Write-Host "Syncing 1m bars..." -ForegroundColor Yellow
aws s3 sync "s3://$S3_BUCKET/bars_1m/" "$LOCAL_DATA_DIR\bars_1m\" --delete
$BARS_COUNT = (Get-ChildItem -Path "$LOCAL_DATA_DIR\bars_1m" -Filter "*.parquet" -Recurse -ErrorAction SilentlyContinue).Count
Write-Host "  Synced $BARS_COUNT bar files" -ForegroundColor Green

# Sync trades
Write-Host "Syncing trades..." -ForegroundColor Yellow
aws s3 sync "s3://$S3_BUCKET/trades/" "$LOCAL_DATA_DIR\trades\" --delete
$TRADES_COUNT = (Get-ChildItem -Path "$LOCAL_DATA_DIR\trades" -Filter "*.parquet" -Recurse -ErrorAction SilentlyContinue).Count
Write-Host "  Synced $TRADES_COUNT trade files" -ForegroundColor Green

# Sync extended bars
Write-Host "Syncing extended bars..." -ForegroundColor Yellow
try {
    aws s3 sync "s3://$S3_BUCKET/extended_bars/" "$LOCAL_DATA_DIR\extended_bars\" --delete 2>$null
} catch {
    # Ignore if bucket path doesn't exist
}
$EXT_COUNT = (Get-ChildItem -Path "$LOCAL_DATA_DIR\extended_bars" -Filter "*.parquet" -Recurse -ErrorAction SilentlyContinue).Count
Write-Host "  Synced $EXT_COUNT extended bar files" -ForegroundColor Green

# Calculate total size
$TOTAL_SIZE = "{0:N2} MB" -f ((Get-ChildItem -Path $LOCAL_DATA_DIR -Recurse | Measure-Object -Property Length -Sum).Sum / 1MB)
Write-Host ""
Write-Host "Total synced: $TOTAL_SIZE" -ForegroundColor Green

# Verify if requested
if ($Verify) {
    Write-Host ""
    Write-Host "Running verification..." -ForegroundColor Yellow

    $pythonScript = @"
import pyarrow.parquet as pq
import os
from pathlib import Path

data_dir = Path(r'$LOCAL_DATA_DIR')
errors = []

for parquet_file in data_dir.rglob('*.parquet'):
    try:
        table = pq.read_table(str(parquet_file))
        print(f'  OK {parquet_file.name}: {table.num_rows} rows')
    except Exception as e:
        errors.append(f'{parquet_file}: {e}')
        print(f'  FAIL {parquet_file.name}: {e}')

if errors:
    print(f'\nVerification found {len(errors)} errors')
    exit(1)
else:
    print('\nVerification passed!')
"@

    python -c $pythonScript
    if ($LASTEXITCODE -ne 0) {
        Write-Host "Verification failed!" -ForegroundColor Red
        exit 1
    }
}

Write-Host ""
Write-Host "=============================================="
Write-Host "Sync complete!" -ForegroundColor Green
Write-Host "=============================================="
Write-Host ""
Write-Host "Summary:"
Write-Host "  Bars:          $BARS_COUNT files"
Write-Host "  Trades:        $TRADES_COUNT files"
Write-Host "  Extended bars: $EXT_COUNT files"
Write-Host "  Total size:    $TOTAL_SIZE"

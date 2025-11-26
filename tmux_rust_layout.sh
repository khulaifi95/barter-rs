#!/bin/bash
# Optimal Rust + Multi-Agent tmux layout

SESSION="rust-dev-new"

# Create session with first window
tmux new-session -d -s $SESSION -n main -x 213 -y 54

# Split into 4 panes
tmux split-window -h -t $SESSION:0     # Split vertically (left/right)
tmux split-window -v -t $SESSION:0.0   # Split left pane horizontally
tmux split-window -v -t $SESSION:0.1   # Split right pane horizontally

# Pane 0 (top-left): Claude Code
tmux send-keys -t $SESSION:0.0 "cd ~/projects/barter-rs" Enter
tmux send-keys -t $SESSION:0.0 "echo '🤖 Claude Code - Ready to start with: claude'" Enter

# Pane 1 (top-right): Codex
tmux send-keys -t $SESSION:0.1 "cd ~/projects/barter-rs" Enter
tmux send-keys -t $SESSION:0.1 "echo '🤖 Codex - Ready to start with: codex'" Enter

# Pane 2 (bottom-left): Cargo Output Monitor
tmux send-keys -t $SESSION:0.2 "cd ~/projects/barter-rs" Enter
tmux send-keys -t $SESSION:0.2 "echo '📦 Cargo Output Monitor'" Enter
tmux send-keys -t $SESSION:0.2 "echo 'Run: cargo watch -x test -x build'" Enter

# Pane 3 (bottom-right): Logs & File Watcher
tmux send-keys -t $SESSION:0.3 "cd ~/projects/barter-rs" Enter
tmux send-keys -t $SESSION:0.3 "echo '📊 Monitoring & Logs'" Enter
tmux send-keys -t $SESSION:0.3 "echo 'Useful commands:'" Enter
tmux send-keys -t $SESSION:0.3 "echo '  - git diff --stat'" Enter
tmux send-keys -t $SESSION:0.3 "echo '  - cargo test -- --nocapture 2>&1 | tee test.log'" Enter

# Color code the panes
tmux select-pane -t $SESSION:0.0 -P 'bg=colour234'  # Claude - dark
tmux select-pane -t $SESSION:0.1 -P 'bg=colour235'  # Codex - slightly lighter
tmux select-pane -t $SESSION:0.2 -P 'bg=colour22'   # Cargo - dark green
tmux select-pane -t $SESSION:0.3 -P 'bg=colour236'  # Logs - gray

# Set pane titles (requires tmux 2.6+)
tmux select-pane -t $SESSION:0.0 -T "Claude"
tmux select-pane -t $SESSION:0.1 -T "Codex"
tmux select-pane -t $SESSION:0.2 -T "Cargo"
tmux select-pane -t $SESSION:0.3 -T "Monitor"

# Focus on Claude pane
tmux select-pane -t $SESSION:0.0

echo "✅ Rust dev session '$SESSION' created!"
echo "📌 Attach with: tmux attach -t $SESSION"
echo ""
echo "Pane layout:"
echo "  ┌──────────────┬──────────────┐"
echo "  │ Claude (0.0) │ Codex (0.1)  │"
echo "  ├──────────────┼──────────────┤"
echo "  │ Cargo (0.2)  │ Monitor(0.3) │"
echo "  └──────────────┴──────────────┘"

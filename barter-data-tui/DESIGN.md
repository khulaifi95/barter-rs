# Barter Data TUI - Design Guide

## Color Palette

The TUI uses a modern, cyberpunk-inspired color scheme optimized for terminal visibility:

### Primary Colors
- **Background**: `RGB(15, 15, 25)` - Deep dark blue-black
- **Alt Background**: `RGB(18, 18, 28)` - Slightly lighter for contrast
- **Border Purple**: `RGB(138, 43, 226)` - Vibrant purple for main borders

### Status Colors
- **Connected**: `RGB(0, 255, 127)` - Bright spring green
- **Disconnected**: `RGB(255, 69, 58)` - Vivid red-orange
- **Warning**: `RGB(255, 215, 0)` - Bright gold

### Exchange-Specific Colors
- **OKX**: `RGB(0, 120, 255)` - Blue
- **Binance**: `RGB(240, 185, 11)` - Gold/Yellow
- **Bybit**: `RGB(255, 92, 0)` - Orange

### Panel Colors
- **Liquidations Border**: `RGB(255, 69, 58)` - Red (danger)
- **Open Interest Border**: `RGB(100, 149, 237)` - Cornflower blue
- **CVD Border**: `RGB(255, 105, 180)` - Hot pink

### Data Colors
- **Buy/Long**: `RGB(0, 255, 127)` - Spring green
- **Sell/Short**: `RGB(255, 69, 58)` - Red-orange
- **Neutral**: `RGB(255, 215, 0)` - Gold
- **Price**: `RGB(255, 215, 0)` - Gold (emphasis)
- **Quantity**: `RGB(255, 105, 180)` - Hot pink
- **Text Primary**: `RGB(255, 255, 255)` - Pure white
- **Text Secondary**: `RGB(200, 200, 220)` - Light gray
- **Text Tertiary**: `RGB(128, 128, 150)` - Medium gray

## Visual Elements

### Status Bar
- **Border**: Double-line purple border
- **Layout**: Centered with connection status, timestamp, title, and help
- **Icons**: ● (connected), ○ (disconnected), ⏱ (time), ◆ (decorative)

### Liquidations Panel
- **Border**: Rounded red border
- **Title**: ⚡ LIQUIDATIONS FEED with event count
- **Rows**: Alternating background colors for readability
- **Icons**: ▲ (buy liq), ▼ (sell liq)
- **Formatting**:
  - Time: Gray
  - Direction arrow: Bold, color-coded
  - Exchange: Exchange-specific colors in brackets
  - Instrument: Light gray
  - Price: Bold gold
  - Quantity: Hot pink

### Open Interest Panel
- **Border**: Rounded cornflower blue border
- **Title**: 📊 OPEN INTEREST
- **Layout**: Divided sections per exchange
- **Features**:
  - Exchange name in bold bright blue
  - Current value in white
  - Trend arrows: ↑ (up), ↓ (down), — (stable)
  - Percentage change: Color-coded by direction
  - Sparkline chart in blue

### CVD Panel
- **Border**: Rounded hot pink border
- **Title**: 💹 CUMULATIVE VOLUME DELTA
- **Layout**: Divided sections per exchange
- **Features**:
  - Exchange name in bold pink
  - Delta Base: Color-coded (green positive, red negative)
  - Delta Quote: Light gray
  - Buy Pressure Gauge:
    - Background: Dark gray
    - Fill: Color-coded (green >60%, yellow 40-60%, red <40%)
    - Label: White bold text with percentage
  - Sparkline chart in pink

## Typography

### Font Weights
- **Bold**: Used for:
  - Panel titles
  - Direction indicators (BUY/SELL, ▲/▼)
  - Connection status
  - Important values (prices, deltas, percentages)
  - Exchange names in stats panels

- **Regular**: Used for:
  - Most text content
  - Timestamps
  - Secondary information

- **Italic**: Used for:
  - "Waiting for data..." messages

### Text Sizes
All text uses the terminal's default font size. Emphasis is achieved through:
- Color contrast
- Bold modifier
- Spacing and alignment

## Layout

### Main Structure
```
┌─────────────────────────────────────────────────────────────┐
│                     Status Bar (3 lines)                    │
├─────────────────────────────────┬───────────────────────────┤
│                                 │                           │
│   Liquidations (55% width)      │   Stats (45% width)       │
│   - Rolling feed                │   ├──────────────────┤    │
│   - Color-coded rows            │   │ Open Interest    │    │
│   - Alternating backgrounds     │   │ - Exchange info  │    │
│   - Bold arrows                 │   │ - Sparklines     │    │
│                                 │   ├──────────────────┤    │
│                                 │   │ CVD              │    │
│                                 │   │ - Deltas         │    │
│                                 │   │ - Gauge bars     │    │
│                                 │   │ - Sparklines     │    │
└─────────────────────────────────┴───────────────────────────┘
```

### Spacing
- **Panel Margin**: 1 character on all sides
- **Internal Padding**: 2 characters from border
- **Row Spacing**: Varies by content, alternating backgrounds for readability
- **Section Dividers**: Automatic based on data count

### Border Styles
- **Status Bar**: `BorderType::Double` - Emphasizes importance
- **All Panels**: `BorderType::Rounded` - Modern, friendly appearance

## Animation & Updates

### Refresh Rate
- **250ms** - Fast enough for real-time feel, efficient for CPU

### Visual Feedback
- **Connection Status**: Instant color change on connect/disconnect
- **Data Updates**: Smooth updates with no flicker
- **Sparklines**: Scroll left as new data arrives
- **Gauges**: Smooth transitions (handled by terminal)

## Accessibility

### Color Blindness Considerations
- **Red/Green**: Also uses symbols (▲/▼) and positioning
- **Color + Text**: All information conveyed through both color AND text
- **Contrast**: High contrast ratios for readability

### Information Hierarchy
1. **Critical**: Connection status, liquidation events
2. **Important**: Current values, trends
3. **Supporting**: Historical sparklines, percentages

## Design Principles

### 1. Information Density
- Maximize data visibility without clutter
- Use alternating backgrounds for row separation
- Clear visual hierarchy with colors and weights

### 2. Real-Time Focus
- Most recent data at top (reversed chronological)
- Immediate visual feedback on state changes
- Smooth, flicker-free updates

### 3. Cyberpunk Aesthetic
- Dark backgrounds with neon accents
- Vibrant borders and highlights
- Modern rounded corners
- Tech-inspired icons (⚡, 📊, 💹, ●, ⏱)

### 4. Terminal Optimization
- Uses RGB colors for modern terminals
- Graceful degradation for 256-color terminals
- Efficient rendering with bounded data structures
- No unnecessary redraws

## Future Enhancements

Potential design improvements:
- [ ] Add mini price charts for each instrument
- [ ] Implement tabs for different timeframes
- [ ] Add configurable color themes
- [ ] Mouse interaction for panel resizing
- [ ] Animated transitions between states
- [ ] Volume histogram visualization
- [ ] Alert system with visual notifications
- [ ] Dark/light theme toggle
- [ ] Custom color picker in settings

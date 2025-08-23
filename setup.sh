#!/bin/bash

# Setup script for Discord Music Bot dependencies

echo "🎵 Discord Music Bot Setup Script"
echo "================================="

# Detect OS
if [[ "$OSTYPE" == "darwin"* ]]; then
    echo "📱 macOS detected"

    # Check if Homebrew is installed
    if ! command -v brew &> /dev/null; then
        echo "❌ Homebrew not found. Installing Homebrew..."
        /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
    fi

    echo "📦 Installing dependencies via Homebrew..."
    brew install ffmpeg opus pkg-config cmake python3

    echo "🐍 Installing/Updating yt-dlp..."
    # Use brew's Python to ensure compatibility
    brew install yt-dlp
    # Or alternatively:
    # python3 -m pip install -U yt-dlp

elif [[ "$OSTYPE" == "linux-gnu"* ]]; then
    echo "🐧 Linux detected"

    # Detect package manager
    if command -v apt-get &> /dev/null; then
        echo "📦 Installing dependencies via apt..."
        sudo apt-get update
        sudo apt-get install -y ffmpeg libopus-dev pkg-config cmake python3-pip build-essential
    elif command -v yum &> /dev/null; then
        echo "📦 Installing dependencies via yum..."
        sudo yum install -y ffmpeg opus-devel pkg-config cmake python3-pip
    elif command -v pacman &> /dev/null; then
        echo "📦 Installing dependencies via pacman..."
        sudo pacman -S ffmpeg opus pkg-config cmake python-pip
    fi

    echo "🐍 Installing/Updating yt-dlp..."
    python3 -m pip install -U yt-dlp

elif [[ "$OSTYPE" == "msys" ]] || [[ "$OSTYPE" == "cygwin" ]] || [[ "$OSTYPE" == "win32" ]]; then
    echo "🪟 Windows detected"
    echo "Please install the following manually:"
    echo "1. FFmpeg: https://ffmpeg.org/download.html"
    echo "2. Python: https://www.python.org/downloads/"
    echo "3. After Python is installed, run: pip install -U yt-dlp"
    exit 1
fi

echo ""
echo "✅ Verifying installations..."
echo "================================="

# Check FFmpeg
if command -v ffmpeg &> /dev/null; then
    FFMPEG_VERSION=$(ffmpeg -version | head -n1)
    echo "✅ FFmpeg: $FFMPEG_VERSION"
else
    echo "❌ FFmpeg not found!"
fi

# Check yt-dlp
if command -v yt-dlp &> /dev/null; then
    YTDLP_VERSION=$(yt-dlp --version)
    echo "✅ yt-dlp: $YTDLP_VERSION"
else
    echo "❌ yt-dlp not found!"
fi

# Check pkg-config
if command -v pkg-config &> /dev/null; then
    echo "✅ pkg-config: $(pkg-config --version)"
else
    echo "❌ pkg-config not found!"
fi

# Check Opus
if pkg-config --exists opus; then
    echo "✅ Opus: $(pkg-config --modversion opus)"
else
    echo "❌ Opus library not found!"
fi

echo ""
echo "🔧 Setting up environment..."
echo "================================="

# Create .env file if it doesn't exist
if [ ! -f .env ]; then
    echo "Creating .env file..."
    cat > .env << EOL
# Discord Bot Token
DISCORD_TOKEN=your_discord_bot_token_here

# Optional: YouTube API Key (for better search)
# YOUTUBE_API_KEY=your_youtube_api_key_here
EOL
    echo "✅ Created .env file. Please add your Discord bot token!"
else
    echo "✅ .env file already exists"
fi

echo ""
echo "🎵 Testing yt-dlp audio extraction..."
echo "================================="

# Test yt-dlp with a short video
TEST_URL="https://www.youtube.com/watch?v=jNQXAC9IVRw"
echo "Testing with 'Me at the zoo' (first YouTube video)..."

if yt-dlp -f bestaudio --get-url "$TEST_URL" &> /dev/null; then
    echo "✅ yt-dlp audio extraction working!"
else
    echo "❌ yt-dlp audio extraction failed. Trying to fix..."

    # Update yt-dlp
    echo "Updating yt-dlp..."
    if [[ "$OSTYPE" == "darwin"* ]]; then
        brew upgrade yt-dlp
    else
        python3 -m pip install -U yt-dlp
    fi
fi

echo ""
echo "📝 Configuration Tips:"
echo "================================="
echo "1. Make sure to add your Discord bot token to the .env file"
echo "2. If audio playback fails, try updating yt-dlp: yt-dlp -U"
echo "3. For better performance, consider using a YouTube API key"
echo "4. Run 'cargo build --release' for optimized performance"
echo ""
echo "🚀 Setup complete! You can now run: cargo run"

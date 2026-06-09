TARGET ?=
TARGET_FLAG = $(if $(TARGET),--target=$(TARGET),)

# Build the cronet libraries (.a/.so/.dll) and collect them into lib/ + include/.
build:
	go run -v ./cmd/build-naive build $(TARGET_FLAG)
	go run -v ./cmd/build-naive package $(TARGET_FLAG)

# Convenience target for all Apple platforms.
apple:
	TARGET="ios/arm64,ios/arm64/simulator,ios/amd64/simulator,tvos/arm64,tvos/arm64/simulator,tvos/amd64/simulator,darwin/arm64,darwin/amd64" make

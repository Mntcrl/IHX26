
SCHEME     := XRPL_Galaxy
CONFIG     := Debug
SDK        := iphoneos
BUILD_DIR  := $(CURDIR)/build
DEVICE     := E34AE83F-D43C-510A-94EA-8E032BCD853E
APP        := $(BUILD_DIR)/$(CONFIG)-iphoneos/$(SCHEME).app

AMBIENT_BACKEND_URL ?=
AMBIENT_HALF_WINDOW_SECONDS ?= 5
AMBIENT_POST_WINDOW_SECONDS ?= 0.35
APP_ATTEST_ENVIRONMENT ?= development
CAPTURE_SWIFT_OPTIMIZATION ?= -O
POV_COORDINATOR_URL ?= http://10.164.228.104:8088

ifneq ($(strip $(AMBIENT_BACKEND_URL)),)
ATTEST_SETTINGS := CODE_SIGN_ENTITLEMENTS=AppAttest.entitlements APP_ATTEST_ENVIRONMENT="$(APP_ATTEST_ENVIRONMENT)"
endif

.DEFAULT_GOAL := run

.PHONY: all build install launch run clean devices

all: run

build:
	xcodebuild \
		-target $(SCHEME) \
		-sdk $(SDK) \
		-configuration $(CONFIG) \
		-allowProvisioningUpdates \
		SWIFT_OPTIMIZATION_LEVEL="$(CAPTURE_SWIFT_OPTIMIZATION)" \
		AMBIENT_BACKEND_URL="$(AMBIENT_BACKEND_URL)" \
		AMBIENT_HALF_WINDOW_SECONDS="$(AMBIENT_HALF_WINDOW_SECONDS)" \
		AMBIENT_POST_WINDOW_SECONDS="$(AMBIENT_POST_WINDOW_SECONDS)" \
		POV_COORDINATOR_URL="$(POV_COORDINATOR_URL)" \
		$(ATTEST_SETTINGS) \
		build

install: build
	xcrun devicectl device install app \
		--device $(DEVICE) \
		"$(APP)"

launch: install
	xcrun devicectl device process launch \
		--device $(DEVICE) \
		--terminate-existing \
		"$$("/usr/libexec/PlistBuddy" -c 'Print CFBundleIdentifier' "$(APP)/Info.plist")"

run: launch

clean:
	rm -rf "$(BUILD_DIR)"

devices:
	xcrun devicectl list devices


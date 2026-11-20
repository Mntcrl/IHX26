ROOT := ../..
.DEFAULT_GOAL := run

.PHONY: build install run logs pull-fixtures
build install run:
	$(ROOT)/scripts/with-slot.sh $(ROOT)/scripts/ios.sh $@

logs pull-fixtures:
	$(ROOT)/scripts/ios.sh $@

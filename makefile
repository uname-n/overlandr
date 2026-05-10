CARGO    = cargo run --release --
USFS     = shape/national_forest_system_roads/nfsr.shp
REGISTRY = ghcr.io/uname-n/overlandr
PACKAGE  = $(notdir $(REGISTRY))
STATES   = oregon idaho montana arizona
SOURCE   = https://github.com/uname-n/overlandr
VERSION  = $(shell awk -F '"' '/^version = / { print $$2; exit }' Cargo.toml)

.PHONY: all oregon idaho montana arizona build install clean publish prune $(addprefix publish-,$(STATES))

all: build oregon idaho montana arizona

build:
	cargo build --release

install: build
	cargo install --path .

oregon:  bin/oregon.bin
idaho:   bin/idaho.bin
montana: bin/montana.bin
arizona: bin/arizona.bin

bin/:
	mkdir -p bin

bin/oregon.bin: bin/ osm/oregon-*.osm.pbf $(USFS)
	$(CARGO) build osm/oregon-*.osm.pbf --usfs $(USFS) --out $@

bin/idaho.bin: bin/ osm/idaho-*.osm.pbf $(USFS)
	$(CARGO) build osm/idaho-*.osm.pbf --usfs $(USFS) --out $@

bin/montana.bin: bin/ osm/montana-*.osm.pbf $(USFS)
	$(CARGO) build osm/montana-*.osm.pbf --usfs $(USFS) --out $@

bin/arizona.bin: bin/ osm/arizona-*.osm.pbf $(USFS)
	$(CARGO) build osm/arizona-*.osm.pbf --usfs $(USFS) --out $@

publish: $(addprefix publish-,$(STATES)) prune

$(addprefix publish-,$(STATES)): publish-%: bin/%.bin
	docker buildx build \
		--build-arg STATE=$* \
		--annotation "index:org.opencontainers.image.source=$(SOURCE)" \
		--annotation "index:org.opencontainers.image.title=overlandr" \
		--annotation "index:org.opencontainers.image.description=Overland route planning server for $*." \
		--push \
		-t $(REGISTRY):$* \
		-t $(REGISTRY):$*-$(VERSION) \
		-f dockerfile .

docker: $(addprefix docker-,$(STATES)) prune

$(addprefix docker-,$(STATES)): docker-%: bin/%.bin
	docker buildx build \
		--build-arg STATE=$* \
		--annotation "index:org.opencontainers.image.source=$(SOURCE)" \
		--annotation "index:org.opencontainers.image.title=overlandr" \
		--annotation "index:org.opencontainers.image.description=Overland route planning server for $*." \
		-t overlandr:$* \
		-t overlandr:$*-$(VERSION) \
		-f dockerfile .
prune:
	@gh api "/user/packages/container/$(PACKAGE)/versions" --paginate \
		--jq '.[] | select(.metadata.container.tags | length == 0) | .id' \
		| while read -r id; do gh api -X DELETE "/user/packages/container/$(PACKAGE)/versions/$$id" > /dev/null; done

clean:
	rm -f bin/oregon.bin bin/idaho.bin bin/montana.bin bin/arizona.bin

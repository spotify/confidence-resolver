.PHONY: install test lint proto clean e2e wasm format venv

VENV := .venv
PYTHON := $(VENV)/bin/python
PIP := $(VENV)/bin/pip

$(VENV)/bin/activate:
	python3 -m venv $(VENV)
	$(PIP) install --upgrade pip

venv: $(VENV)/bin/activate

install: venv
	$(PIP) install -e ".[dev]"

test:
	$(PYTHON) -m pytest tests/ -v --ignore=tests/e2e_test.py --ignore=tests/flaglogs_e2e_test.py

e2e:
	$(PYTHON) -m pytest tests/e2e_test.py tests/flaglogs_e2e_test.py -v

lint:
	$(PYTHON) -m black --check src/ tests/
	$(PYTHON) -m flake8 src/ tests/
	$(PYTHON) -m mypy src/

format:
	$(PYTHON) -m black src/ tests/

proto:
	$(PYTHON) -m grpc_tools.protoc \
		-I../proto \
		--python_out=src/confidence_openfeature/proto \
		--grpc_python_out=src/confidence_openfeature/proto \
		../proto/confidence/flags/resolver/v1/*.proto \
		../proto/confidence/wasm/*.proto

wasm:
	mkdir -p resources/wasm
	cp ../go/confidence/internal/local_resolver/assets/confidence_resolver.wasm resources/wasm/

clean:
	rm -rf build/ dist/ *.egg-info .pytest_cache .mypy_cache
	find . -type d -name __pycache__ -exec rm -rf {} +

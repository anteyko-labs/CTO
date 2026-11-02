#!/usr/bin/env bash
# Проверка deploy.sh в контейнере ubuntu:24.04 (Docker на Маке): deploy/cloud/test/run.sh
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
docker run --rm -v "$here:/src:ro" ubuntu:24.04 bash -c '
  apt-get -qq update > /dev/null && apt-get -qq install -y git > /dev/null 2>&1
  install -D -m 755 /src/deploy.sh /opt/avtodom/scripts/deploy.sh
  bash /src/test/stubs.sh && bash /src/test/test-deploy.sh'

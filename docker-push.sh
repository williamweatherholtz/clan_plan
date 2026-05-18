#!/usr/bin/env bash
set -euo pipefail

IMAGE="${IMAGE:-docker.io/YOURUSER/clanplan}"
TAG="${1:-latest}"

echo "Building $IMAGE:$TAG ..."
docker build -t "$IMAGE:$TAG" .

if [[ "$TAG" != "latest" ]]; then
  docker tag "$IMAGE:$TAG" "$IMAGE:latest"
fi

echo "Pushing $IMAGE:$TAG ..."
docker push "$IMAGE:$TAG"

if [[ "$TAG" != "latest" ]]; then
  echo "Pushing $IMAGE:latest ..."
  docker push "$IMAGE:latest"
fi

echo "Done."

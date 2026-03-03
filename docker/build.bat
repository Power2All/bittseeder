@echo off

docker build --no-cache -t power2all/bittseeder:v0.1.0 -t power2all/bittseeder:latest .
docker push power2all/bittseeder:v0.1.0
docker push power2all/bittseeder:latest
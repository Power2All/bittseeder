@echo off

docker build --no-cache -t power2all/bittseeder:v0.1.1 -t power2all/bittseeder:latest .
docker push power2all/bittseeder:v0.1.1
docker push power2all/bittseeder:latest
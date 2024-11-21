#!/bin/bash

EXAMPLES="$(dirname "${BASH_SOURCE[0]}")"

trap "kill 0" SIGINT
trap "kill 0" SIGQUIT
cd "$EXAMPLES/.."
nginx -p . -c "$EXAMPLES/nginx.conf" &
cargo run & 
wait 

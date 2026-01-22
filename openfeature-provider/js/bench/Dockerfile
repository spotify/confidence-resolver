FROM node:20-alpine3.22

WORKDIR /app

# Install runtime deps
COPY openfeature-provider/js/bench/package.json ./package.json
RUN npm install --omit=dev

# Copy provider dist (JS + WASM)
COPY openfeature-provider/js/packages/server-provider/dist ./dist

# Copy entire bench directory so relative imports (../dist) remain valid
COPY openfeature-provider/js/bench ./bench

ENV NODE_ENV=production

ENTRYPOINT ["node", "bench/bench.mjs"]



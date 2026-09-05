FROM node:24-alpine
WORKDIR /app
COPY services/package.json ./
RUN npm install --omit=dev --no-audit --no-fund
# todos los modulos compartidos, no solo el que existia al escribir esto
COPY services/*.ts ./
COPY services/checkout ./checkout
ENV AXON_SERVICE=checkout
# Node 24 ejecuta TypeScript directo: sin paso de build, sin bundler.
CMD ["node", "checkout/index.ts"]

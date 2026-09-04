FROM node:24-alpine
WORKDIR /app
COPY services/package.json ./
RUN npm install --omit=dev --no-audit --no-fund
# todos los modulos compartidos, no solo el que existia al escribir esto
COPY services/*.ts ./
COPY services/orders ./orders
ENV AXON_SERVICE=orders
# Node 24 ejecuta TypeScript directo: sin paso de build, sin bundler.
CMD ["node", "orders/index.ts"]

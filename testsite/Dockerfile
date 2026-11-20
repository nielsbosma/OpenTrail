# The site that gets recorded: static scenario pages and a few API endpoints. Not for production.
FROM node:26-alpine@sha256:c6ad9e9c1d80f6eb607c26b4b0a132ff7a2344fad1c78ea3baa2a7d4c8de6d94
WORKDIR /site
COPY server.mjs ./
COPY site site
ENV PORT=8092
EXPOSE 8092
USER node
CMD ["node", "server.mjs"]

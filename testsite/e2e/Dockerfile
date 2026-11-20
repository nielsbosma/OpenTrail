# Playwright with its browsers, running the scenario tests against the test site and OpenTrail.
FROM mcr.microsoft.com/playwright:v1.64.1-noble
WORKDIR /e2e
COPY package.json ./
RUN npm install --no-audit --no-fund
COPY playwright.config.ts ./
COPY tests tests
CMD ["npx", "playwright", "test"]

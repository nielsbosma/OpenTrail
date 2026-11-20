import { defineConfig, devices } from '@playwright/test';

export default defineConfig({
  testDir: 'tests',
  timeout: 120_000,
  workers: 1,
  reporter: [['list']],
  use: { ...devices['Desktop Chrome'], trace: 'retain-on-failure' },
});

import { test, expect } from '@playwright/test';

test('smoke test - Grafana home page loads', async ({ page }) => {
  await page.goto('/');
  
  await expect(page.getByText('Welcome to Grafana')).toBeVisible();
});

import { test, expect } from '@playwright/test';

// Covers the @grafana/experimental -> @grafana/plugin-ui `SQLEditor` swap: opens a panel
// using this plugin's datasource, switches from the query builder to the raw SQL editor,
// and confirms the Monaco-based editor renders and accepts typed input.
test('query editor can switch to the raw SQL editor and accept input', async ({ page, request }) => {
  // Provision a throwaway datasource instance of this plugin so the query editor can be opened.
  // It doesn't need to reach a real FlightSQL backend for this test - we're only verifying
  // that the query editor UI renders and the SQL editor accepts input.
  const dsName = `Micromegas e2e ${Date.now()}`;
  const createResp = await request.post('/api/datasources', {
    data: {
      name: dsName,
      type: 'micromegas-micromegas-datasource',
      access: 'proxy',
      jsonData: { host: 'localhost:50051' },
    },
  });
  expect(createResp.ok()).toBeTruthy();
  const { datasource } = await createResp.json();

  try {
    await page.goto('/dashboard/new');
    await page.getByRole('button', { name: 'Add visualization' }).click();

    // The builder view renders by default once the query editor is showing.
    const editSqlButton = page.getByRole('button', { name: 'Edit SQL' });

    // Grafana shows a picker to choose a data source when opening a new panel. Wait for
    // either the picker or the query editor itself, then click through the picker if it's
    // the one that showed up.
    const dialog = page.getByRole('dialog', { name: 'Select data source' });
    await expect(dialog.or(editSqlButton)).toBeVisible({ timeout: 15000 });
    if (await dialog.isVisible()) {
      await dialog.getByRole('button', { name: new RegExp(dsName) }).click({ timeout: 10000 });
    }

    // Confirm the query editor controls are showing.
    await expect(editSqlButton).toBeVisible({ timeout: 15000 });

    // Switch from the builder view to the raw SQL editor.
    await editSqlButton.click();
    await page.getByRole('button', { name: 'Switch' }).click();

    const editor = page.locator('.monaco-editor').first();
    await expect(editor).toBeVisible();
    await editor.click();
    await page.keyboard.type('SELECT 1');

    await expect(editor).toContainText('SELECT 1');
  } finally {
    await request.delete(`/api/datasources/uid/${datasource.uid}`);
  }
});

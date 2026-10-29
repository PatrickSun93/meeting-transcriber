const fs = require('fs');
const path = require('path');
const { buildManifest, parseUrl } = require('../scripts/package-teams-app');

describe('Teams app package', () => {
  test('manifest uses the given URL everywhere', () => {
    const manifest = buildManifest(new URL('https://abc.example.dev'));
    expect(manifest.staticTabs[0].contentUrl).toBe('https://abc.example.dev/tab');
    expect(manifest.configurableTabs[0].configurationUrl).toBe('https://abc.example.dev/config');
    expect(manifest.validDomains).toEqual(['abc.example.dev']);
    expect(JSON.stringify(manifest)).not.toMatch(/{{/);
  });

  test('manifest has the fields Teams requires for upload', () => {
    const manifest = buildManifest(new URL('https://localhost:3001'));
    expect(manifest.id).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/);
    expect(manifest.validDomains).toEqual(['localhost:3001']);
    expect(manifest.devicePermissions).toContain('media');
    expect(manifest.configurableTabs[0].context).toContain('meetingSidePanel');
    expect(manifest.name.short.length).toBeLessThanOrEqual(30);
    expect(manifest.description.short.length).toBeLessThanOrEqual(80);
  });

  test('icons are the sizes Teams requires', () => {
    const size = (file) => {
      const png = fs.readFileSync(path.join(__dirname, '../teams-app', file));
      return [png.readUInt32BE(16), png.readUInt32BE(20)];
    };
    expect(size('color.png')).toEqual([192, 192]);
    expect(size('outline.png')).toEqual([32, 32]);
  });

  test('only https URLs are accepted', () => {
    expect(() => parseUrl(['--url', 'http://localhost:3001'])).toThrow(/https/);
    expect(parseUrl(['--url', 'https://x.test:8443']).host).toBe('x.test:8443');
  });
});

const fs = require('fs');
const path = require('path');

module.exports = async function(context) {
    const exePath = path.join(context.appOutDir, `${context.packager.appInfo.productFilename}.exe`);
    if (!fs.existsSync(exePath)) return; // Windows-only step; skip for Linux targets.
    const { rcedit } = await import('rcedit'); // rcedit is ESM; load dynamically in CommonJS.
    const pkg = context.packager.appInfo;
    await rcedit(exePath, {
        icon: path.resolve(__dirname, '..', 'assets', 'icon.ico'),
        'product-version': pkg.version,
        'file-version': pkg.version,
        'version-string': {
            ProductName: 'Advanced PDF Reader',
            FileDescription: 'Advanced PDF Reader',
            CompanyName: 'YAMiN HOSSAIN',
            LegalCopyright: 'Copyright (c) 2026 YAMiN HOSSAIN',
            OriginalFilename: 'Advanced PDF Reader.exe',
        }
    });
    console.log('  • exe patched with icon & metadata');
};

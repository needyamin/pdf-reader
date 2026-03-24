const { rcedit } = require('rcedit');
const path = require('path');

const exePath = path.join(__dirname, '..', 'out', 'win-unpacked', 'Advanced PDF Reader.exe');
const icoPath = path.resolve(__dirname, '..', 'assets', 'icon.ico');

rcedit(exePath, {
    'icon': icoPath,
    'product-version': '1.0.0',
    'file-version': '1.0.0',
    'version-string': {
        ProductName: 'Advanced PDF Reader',
        FileDescription: 'Advanced PDF Reader',
        CompanyName: 'YAMiN HOSSAIN',
        LegalCopyright: 'Copyright (c) 2026 YAMiN HOSSAIN',
        OriginalFilename: 'Advanced PDF Reader.exe',
    }
}).then(() => {
    console.log('  • patched exe with icon and product info');
}).catch(err => {
    console.error('Failed to patch exe:', err);
    process.exit(1);
});

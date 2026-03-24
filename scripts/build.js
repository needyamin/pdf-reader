const { execSync } = require('child_process');
const { rcedit } = require('rcedit');
const path = require('path');
const fs = require('fs');

process.env.CSC_IDENTITY_AUTO_DISCOVERY = 'false';
const run = (cmd) => execSync(cmd, { stdio: 'inherit', cwd: path.join(__dirname, '..') });
const root = path.join(__dirname, '..');
const exePath = path.join(root, 'out', 'win-unpacked', 'Advanced PDF Reader.exe');

(async () => {
    console.log('Step 1: Package app...');
    run('npx electron-builder --win --dir');

    console.log('Step 2: Patch exe with icon and metadata...');
    await rcedit(exePath, {
        icon: path.join(root, 'assets', 'icon.ico'),
        'product-version': '1.0.0',
        'file-version': '1.0.0',
        'version-string': {
            ProductName: 'Advanced PDF Reader',
            FileDescription: 'Advanced PDF Reader',
            CompanyName: 'YAMiN HOSSAIN',
            LegalCopyright: 'Copyright (c) 2026 YAMiN HOSSAIN',
            OriginalFilename: 'Advanced PDF Reader.exe',
        }
    });
    console.log('  • exe patched successfully');

    console.log('Step 3: Sign exe with certificate...');
    const certPathAbs = path.join(root, 'cert.pfx');
    try {
        const psCmd = `$cert = New-Object System.Security.Cryptography.X509Certificates.X509Certificate2('${certPathAbs.replace(/\\/g, '\\\\')}','PdfReader2026!'); Set-AuthenticodeSignature -FilePath '${exePath.replace(/\\/g, '\\\\')}' -Certificate $cert -TimestampServer 'http://timestamp.digicert.com'`;
        run(`powershell -ExecutionPolicy Bypass -Command "${psCmd}"`);
        console.log('  • exe signed successfully');
    } catch (e) {
        console.log('  • signing skipped (non-critical)');
    }

    console.log('Step 4: Build installer...');
    run('npx electron-builder --win nsis --publish never --prepackaged out/win-unpacked');

    console.log('Done! Output: out/Advanced PDF Reader Setup 1.0.0.exe');
})();

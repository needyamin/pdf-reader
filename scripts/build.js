const { execSync } = require('child_process');
const { rcedit } = require('rcedit');
const path = require('path');
const fs = require('fs');

const root = path.join(__dirname, '..');
const pkg = require(path.join(root, 'package.json'));
const VER = pkg.version;
const APP = pkg.build.productName;
const exeName = `${APP}.exe`;
const setupName = `${APP} Setup ${VER}.exe`;
const outDir = path.join(root, 'out');
const exePath = path.join(outDir, 'win-unpacked', exeName);
const setupPath = path.join(outDir, setupName);
const certPath = path.join(root, 'cert.pfx');
const signScript = path.join(__dirname, 'sign.ps1');

process.env.CSC_IDENTITY_AUTO_DISCOVERY = 'false';

const run = (cmd) => execSync(cmd, { stdio: 'inherit', cwd: root });

const sign = (filePath) => {
    if (!fs.existsSync(certPath) || !fs.existsSync(filePath)) return;
    try {
        run(`powershell -ExecutionPolicy Bypass -File "${signScript}" -ExePath "${filePath}"`);
        console.log(`  • signed: ${path.basename(filePath)}`);
    } catch (e) {
        console.log(`  • sign skipped: ${path.basename(filePath)}`);
    }
};

const patch = async (filePath) => {
    await rcedit(filePath, {
        icon: path.join(root, 'assets', 'icon.ico'),
        'product-version': VER,
        'file-version': VER,
        'version-string': {
            ProductName: APP,
            FileDescription: APP,
            CompanyName: 'YAMiN HOSSAIN',
            LegalCopyright: 'Copyright (c) 2026 YAMiN HOSSAIN',
            OriginalFilename: exeName,
        }
    });
};

(async () => {
    try {
        console.log(`\n  Building ${APP} v${VER}\n`);

        // Clean old output
        if (fs.existsSync(outDir)) {
            console.log('Cleaning old build...');
            fs.rmSync(outDir, { recursive: true, force: true });
        }

        console.log('Step 1/4: Compile & Build...');
        run('npx tsc');
        run('npx electron-builder --win --publish never');
        console.log('  • done');

        console.log('Step 2/4: Patch exe...');
        await patch(exePath);
        console.log('  • done');

        console.log('Step 3/4: Sign exe...');
        sign(exePath);

        console.log('Step 4/4: Sign installer...');
        sign(setupPath);

        console.log(`\n  Done! ${setupName} ready in out/\n`);
    } catch (e) {
        console.error('\nBuild failed:', e.message);
        process.exit(1);
    }
})();

"""Verify source archives carry optional native icon dependencies."""
import pathlib, shutil, subprocess, tarfile, tempfile
with tempfile.TemporaryDirectory(prefix='xenia-archive-test-') as tmp:
    root=pathlib.Path(tmp)
    for directory in ('.github','src','web','apps','docs','scripts','tests','script'):
        (root/directory).mkdir()
    for name in ('Cargo.toml','Cargo.lock','LICENSE','README.md','.gitignore'):
        (root/name).write_text('synthetic fixture')
    shutil.copy2('scripts/make-source-archive.sh',root/'scripts/make-source-archive.sh')
    for icons in (False, True):
        if icons:
            (root/'assets').mkdir(); (root/'assets/xenia.ico').write_bytes(b'synthetic icon')
        subprocess.run(['scripts/make-source-archive.sh'],cwd=root,check=True,stdout=subprocess.DEVNULL)
        with tarfile.open(root/'dist/xenia-source.tar.gz') as archive:
            assert ('assets/xenia.ico' in archive.getnames()) == icons
            if icons: assert archive.extractfile('assets/xenia.ico').read()==b'synthetic icon'
    print('Source archive includes optional native icon assets: PASS')

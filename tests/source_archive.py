"""Verify source archives carry optional native icon dependencies."""
import os, pathlib, shutil, subprocess, sys, tarfile, tempfile
with tempfile.TemporaryDirectory(prefix='xenia-archive-test-') as tmp:
    root=pathlib.Path(tmp)
    for directory in ('.github','src','web','apps','docs','scripts','tests','script'):
        (root/directory).mkdir()
    for name in ('Cargo.toml','Cargo.lock','package.json','package-lock.json','LICENSE','README.md','.gitignore'):
        (root/name).write_text('synthetic fixture')
    (root/'node_modules').mkdir()
    (root/'node_modules/local-only').write_text('synthetic dependency cache')
    (root/'web/._artifact').write_bytes(b'synthetic resource-fork metadata')
    if hasattr(os, 'setxattr'):
        try:
            os.setxattr(root/'README.md', 'com.xenia.test-owner' if sys.platform == 'darwin' else 'user.xenia-test-owner', b'synthetic account metadata')
        except OSError:
            pass  # Filesystems without xattrs still exercise archive header checks.
    shutil.copy2('scripts/make-source-archive.sh',root/'scripts/make-source-archive.sh')
    for icons in (False, True):
        if icons:
            (root/'assets').mkdir(); (root/'assets/xenia.ico').write_bytes(b'synthetic icon')
        subprocess.run(['scripts/make-source-archive.sh'],cwd=root,check=True,stdout=subprocess.DEVNULL)
        with tarfile.open(root/'dist/xenia-source.tar.gz') as archive:
            assert {'package.json','package-lock.json'}.issubset(archive.getnames())
            assert not any(name.startswith('node_modules') for name in archive.getnames())
            assert not any(pathlib.PurePosixPath(name).name.startswith('._') for name in archive.getnames())
            for member in archive.getmembers():
                assert member.uid == member.gid == 0
                assert member.uname == member.gname == 'root'
                assert not any('xattr' in key.lower() or 'acl' in key.lower() for key in member.pax_headers)
            assert ('assets/xenia.ico' in archive.getnames()) == icons
            if icons: assert archive.extractfile('assets/xenia.ico').read()==b'synthetic icon'
    print('Source archive carries icon assets with generic ownership and no ACL/xattr metadata: PASS')

"""Catch server filename normalization before advertising usable release downloads."""
import importlib.util
from pathlib import Path
import unittest
spec=importlib.util.spec_from_file_location('release_assets',Path(__file__).resolve().parents[1]/'release/check-assets.py')
module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
class ReleaseAssets(unittest.TestCase):
    def test_server_name_normalization_is_detected(self):
        manifest='a'*64+'  ./rust-irrd_0.1.0~beta.2-1_amd64.deb\n'
        with self.assertRaises(ValueError):module.verify(manifest,{'assets':[{'name':'rust-irrd_0.1.0.beta.2-1_amd64.deb','digest':'sha256:'+'a'*64}]})
    def test_checksum_mismatch_or_missing_asset_is_detected(self):
        for assets in [[],[{'name':'package.deb','digest':'sha256:'+'b'*64}]]:
            with self.assertRaises(ValueError):module.verify('a'*64+'  ./package.deb\n',{'assets':assets})
    def test_safe_names_and_server_digests_match(self):
        self.assertEqual(module.verify('a'*64+'  ./package.deb\n',{'assets':[{'name':'package.deb','digest':'sha256:'+'a'*64},{'name':'SHA256SUMS','digest':'sha256:'+'c'*64}]}),1)

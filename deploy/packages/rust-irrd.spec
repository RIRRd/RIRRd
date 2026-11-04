Name:           rust-irrd
Version:        %{package_version}
Release:        %{?package_release}%{!?package_release:1}
Summary:        Rust Internet Routing Registry daemon
License:        MIT
URL:            https://github.com/RIRRd/RIRRd
Source0:        rust-irrd
Source1:        rust-irrd.service
Source2:        irrd.toml
Source3:        rust-irrd.sysusers.conf
Source4:        rust-irrd.tmpfiles.conf
Source5:        copyright
Source6:        LICENSE
Source8:        THIRD_PARTY_NOTICES.md
Source9:        THIRD_PARTY_LICENSES.txt
Requires:       systemd
Requires:       ca-certificates
Requires(post): systemd
Requires(preun): systemd
Requires(postun): systemd
ExclusiveArch:  x86_64 aarch64
%global debug_package %{nil}
%global _build_id_links none

%description
Rust Internet Routing Registry daemon. This test package requires an
operator-provided snapshot and explicit service startup.

%install
install -D -m 0755 "%{SOURCE0}" "%{buildroot}%{_bindir}/rust-irrd"
install -D -m 0644 "%{SOURCE1}" "%{buildroot}/usr/lib/systemd/system/rust-irrd.service"
install -d -m 0750 "%{buildroot}%{_sysconfdir}/rust-irrd"
install -m 0640 "%{SOURCE2}" "%{buildroot}%{_sysconfdir}/rust-irrd/irrd.toml"
install -D -m 0644 "%{SOURCE3}" "%{buildroot}/usr/lib/sysusers.d/rust-irrd.conf"
install -D -m 0644 "%{SOURCE4}" "%{buildroot}/usr/lib/tmpfiles.d/rust-irrd.conf"
install -D -m 0644 "%{SOURCE5}" "%{buildroot}%{_datadir}/licenses/%{name}/copyright"
install -D -m 0644 "%{SOURCE6}" "%{buildroot}%{_datadir}/licenses/%{name}/LICENSE"

install -D -m 0644 "%{SOURCE8}" "%{buildroot}%{_datadir}/licenses/%{name}/THIRD_PARTY_NOTICES.md"
install -D -m 0644 "%{SOURCE9}" "%{buildroot}%{_datadir}/licenses/%{name}/THIRD_PARTY_LICENSES.txt"

%post
set -e
systemd-sysusers rust-irrd.conf
systemd-tmpfiles --create rust-irrd.conf
if [ -d /run/systemd/system ]; then
    systemctl daemon-reload
fi

%preun
if [ "$1" -eq 0 ]; then
    if [ -d /run/systemd/system ]; then
        systemctl stop rust-irrd.service || exit $?
    fi
    systemctl --root=/ --no-reload disable rust-irrd.service >/dev/null 2>&1 || true
fi

%postun
if [ -d /run/systemd/system ]; then
    systemctl daemon-reload
fi

%files
%{_bindir}/rust-irrd
/usr/lib/systemd/system/rust-irrd.service
/usr/lib/sysusers.d/rust-irrd.conf
/usr/lib/tmpfiles.d/rust-irrd.conf
%dir %attr(0750,root,root) %{_sysconfdir}/rust-irrd
%config(noreplace) %attr(0640,root,root) %{_sysconfdir}/rust-irrd/irrd.toml
%license %{_datadir}/licenses/%{name}/copyright
%license %{_datadir}/licenses/%{name}/LICENSE

%license %{_datadir}/licenses/%{name}/THIRD_PARTY_NOTICES.md

%license %{_datadir}/licenses/%{name}/THIRD_PARTY_LICENSES.txt

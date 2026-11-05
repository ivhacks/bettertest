# Packages the static binary that ./build.sh already built; rpm.sh passes the version and points
# _sourcedir at the repo root. Scriptlets do the same as the .deb's maintainer scripts.

# A stripped static binary has no debug info to split out, and rpm's post-install scripts must leave
# it alone: the .rpm carries the very binary the .deb and the release do.
%global debug_package %{nil}
%global __os_install_post %{nil}

Name:           bettertest
Version:        %{bettertest_version}
Release:        1
Summary:        CI that runs pipelines written in Python
License:        MIT
URL:            https://github.com/ivhacks/bettertest
ExclusiveArch:  x86_64 aarch64
Requires:       python3
Recommends:     (moby-engine or docker-ce)
Recommends:     qemu-system-x86-core
Recommends:     qemu-img
Recommends:     openssh-clients

%description
bettertest is one binary that is both the boss (web UI and API, port 7357) and the worker (runs
tasks, port 7358). Pipelines are Python files in /etc/bettertest/pipedefs. Both run as systemd
services from installation on. docker runs tasks with an image; qemu runs tasks with a vm.

%install
install -D -m 755 %{_sourcedir}/target/%{_arch}-unknown-linux-musl/release/bettertest %{buildroot}%{_bindir}/bettertest
install -D -m 644 %{_sourcedir}/packaging/bettertest-boss.service %{buildroot}%{_unitdir}/bettertest-boss.service
install -D -m 644 %{_sourcedir}/packaging/bettertest-worker.service %{buildroot}%{_unitdir}/bettertest-worker.service
install -D -m 644 %{_sourcedir}/packaging/sysusers.conf %{buildroot}%{_sysusersdir}/bettertest.conf
install -D -m 644 %{_sourcedir}/pipedefs/helloworld.py %{buildroot}%{_sysconfdir}/bettertest/pipedefs/helloworld.py
install -D -m 644 %{_sourcedir}/LICENSE %{buildroot}%{_licensedir}/bettertest/LICENSE

%post
systemd-sysusers bettertest.conf
systemctl daemon-reload
systemctl enable bettertest-boss.service bettertest-worker.service
# restart rather than start, so an upgrade switches to the new binary
systemctl restart bettertest-boss.service bettertest-worker.service

%preun
# $1 is 0 on removal and 1 on upgrade
if [ "$1" -eq 0 ]; then
    systemctl disable --now bettertest-boss.service bettertest-worker.service
fi

%postun
# Run history in /var/tmp/bettertest is data, so removing the package leaves it.
systemctl daemon-reload

%files
%license %{_licensedir}/bettertest/LICENSE
%{_bindir}/bettertest
%{_unitdir}/bettertest-boss.service
%{_unitdir}/bettertest-worker.service
%{_sysusersdir}/bettertest.conf
%dir %{_sysconfdir}/bettertest
%dir %{_sysconfdir}/bettertest/pipedefs
# The admin's to edit: upgrades keep their changes.
%config(noreplace) %{_sysconfdir}/bettertest/pipedefs/helloworld.py

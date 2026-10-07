# This runs inside the Nix wrapper before invoking the release executable.
# Only the first command after global options decides installation ownership.
nix_skip_value=0
for nix_argument do
    if [ "$nix_skip_value" = 1 ]; then nix_skip_value=0; continue; fi
    case "$nix_argument" in
        --api|--token-file) nix_skip_value=1 ;;
        --|--json|--api=*|--token-file=*) ;;
        update|uninstall)
            echo 'Managed by Nix: update the mihomo-server input in your system flake, then run nixos-rebuild switch. Remove the NixOS module to uninstall; user data is retained.' >&2
            exit 1 ;;
        *) break ;;
    esac
done

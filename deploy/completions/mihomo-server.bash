# bash completion for mihomo-server                        -*- shell-script -*-
_mihomo_server() {
    local cur=${COMP_WORDS[COMP_CWORD]} command='' sub='' word i
    for ((i = 1; i < COMP_CWORD; i++)); do
        word=${COMP_WORDS[i]}
        case "$word" in
            --api | --token-file) ((i++)); continue ;;
            -*) continue ;;
        esac
        if [ -z "$command" ]; then command=$word
        elif [ -z "$sub" ]; then sub=$word; fi
    done
    case "${COMP_WORDS[COMP_CWORD-1]}" in
        --token-file) mapfile -t COMPREPLY < <(compgen -f -- "$cur"); return ;;
        --api | --url | --timeout | --name | -n) COMPREPLY=(); return ;;
    esac
    local words='' commands='status start stop restart enable disable info token logs sub proxy mode tun core update uninstall purge serve'
    case "$command" in
        '') words="$commands help --json --api --token-file --help --version" ;;
        sub | subscription | profile)
            case "$sub" in
                '') words='list use update add remove' ;;
                add | import)
                    words='--name --use'
                    case "$cur" in -*) ;; *) mapfile -t COMPREPLY < <(compgen -f -- "$cur"); return ;; esac ;;
                remove | rm) words='--yes' ;;
            esac ;;
        proxy | node)
            case "$sub" in
                '') words='list select test unfix' ;;
                test | delay) words='--url --timeout' ;;
            esac ;;
        mode) [ -n "$sub" ] || words='rule global direct' ;;
        tun) [ -n "$sub" ] || words='on off' ;;
        core)
            case "$sub" in
                '') words='version update' ;;
                update | upgrade) words='--alpha --force' ;;
            esac ;;
        update | upgrade) words='--force' ;;
        uninstall) words='--purge --yes' ;;
        purge) words='--yes' ;;
        help) [ -n "$sub" ] || words=$commands ;;
    esac
    mapfile -t COMPREPLY < <(compgen -W "$words" -- "$cur")
}
complete -F _mihomo_server mihomo-server

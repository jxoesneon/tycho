# bash completion for tycho
_tycho() {
    local cur prev opts
    COMPREPLY=()
    cur="${COMP_WORDS[COMP_CWORD]}"
    prev="${COMP_WORDS[COMP_CWORD-1]}"
    opts="run daemon query test-desktop pull-models --help --version"

    case "${prev}" in
        pull-models)
            COMPREPLY=( $(compgen -W "--force" -- ${cur}) )
            return 0
            ;;
        *)
            ;;
    esac

    COMPREPLY=( $(compgen -W "${opts}" -- ${cur}) )
    return 0
}
complete -F _tycho tycho

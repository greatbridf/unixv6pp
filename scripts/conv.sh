#!/bin/sh

while read -r filename; do
        if file -I "$filename" | grep -q 'utf-8'; then
                continue
        fi

        if ! iconv -f cp936 -t utf-8 "$filename" > "$filename.new"; then
                echo "$filename failed"
                continue
        fi

        mv "$filename.new" "$filename"
done <<< "$(find src -type f)"

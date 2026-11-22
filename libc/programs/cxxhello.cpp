// cxxhello.cpp - the first C++ program on TheTaaJ: its arguments, sorted
// and counted with the standard library, and an exception thrown and
// caught on the way.
//
//   cxxhello pear apple pear fig
//
//   Hello from C++
//   4 words, 3 different, in order: apple fig pear pear
//   pear 2, apple 1, fig 1
//   the longest: apple (5 letters)
//   caught: no word may be longer than 12 letters, and "..." is 13
//   (only with a word that long)

#include <algorithm>
#include <iostream>
#include <map>
#include <stdexcept>
#include <string>
#include <vector>

// Throws if any word is too long; the caller decides what to do about it.
static void check_lengths(const std::vector<std::string> &words)
{
    for (const auto &word : words)
        if (word.size() > 12)
            throw std::length_error("no word may be longer than 12 letters, and \"" + word + "\" is " +
                                    std::to_string(word.size()));
}

int main(int argc, char **argv)
{
    std::cout << "Hello from C++\n";
    std::vector<std::string> words(argv + 1, argv + argc);
    if (words.empty()) {
        std::cout << "give me some words: cxxhello pear apple pear fig\n";
        return 0;
    }

    std::vector<std::string> sorted = words;
    std::sort(sorted.begin(), sorted.end());
    std::map<std::string, int> counts;
    for (const auto &word : words)
        counts[word]++;
    std::cout << words.size() << (words.size() == 1 ? " word, " : " words, ") << counts.size() << " different, in order:";
    for (const auto &word : sorted)
        std::cout << ' ' << word;
    std::cout << '\n';

    // Most common first; equally common in the order they first came.
    std::vector<std::string> seen;
    for (const auto &word : words)
        if (std::find(seen.begin(), seen.end(), word) == seen.end())
            seen.push_back(word);
    std::stable_sort(seen.begin(), seen.end(),
                     [&](const std::string &a, const std::string &b) { return counts[a] > counts[b]; });
    for (std::size_t i = 0; i < seen.size(); i++)
        std::cout << (i ? ", " : "") << seen[i] << ' ' << counts[seen[i]];
    std::cout << '\n';

    auto longest = std::max_element(words.begin(), words.end(),
                                    [](const std::string &a, const std::string &b) { return a.size() < b.size(); });
    std::cout << "the longest: " << *longest << " (" << longest->size() << " letters)\n";

    try {
        check_lengths(words);
    } catch (const std::length_error &e) {
        std::cout << "caught: " << e.what() << '\n';
    }
    return 0;
}

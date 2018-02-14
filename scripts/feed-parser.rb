#
# Just a basic Ruby script that acts as a template for spelunking around feed
# XML files to help find errors that are giving the Rust parser trouble. It'll
# need to be customized for every new use.
#
# Invoke with something like:
#
#     ruby scripts/feed-parser.rb <feed file>
#

require 'nokogiri'
require 'set'

file = ARGV[0] || abort("need file argument")
puts "file = #{file}"

doc = Nokogiri::XML(File.read(file))

guids = Set.new

doc.xpath('//rss/channel/item').each do |item|
  guid = item.at_xpath('guid').content
  puts "Guid = #{guid}"

  if guids.include?(guid)
    puts "Duplicate Guid = #{guid}"
  end

  guids << guid
end

#sorted = titles.sort_by { |t| t.length }.reverse
#puts "Longest title: #{sorted.first} (len: #{sorted.first.length})"

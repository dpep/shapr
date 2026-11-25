class Report
  def render(title, rows)
    puts title
    header = rows.first.keys.map(&:to_s)
    puts header.join(" | ")
    puts "-" * header.join(" | ").size
    puts "end"
  end

  def dispatch(request, verb)
    request.send(verb, title)
  end
end

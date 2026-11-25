class Report
  def render(title, rows)
    puts title
    print_header(rows)
    puts "end"
  end

  def print_header(rows)
    header = rows.first.keys.map(&:to_s)
    puts header.join(" | ")
    puts "-" * header.join(" | ").size
  end

  def dispatch(request, verb)
    request.send(:print_header, title)
  end
end

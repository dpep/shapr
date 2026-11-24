class Fetcher
  def pad(text, width = 10, fill = " ")
    text.ljust(width, fill)
  end

  def go(x)
    pad(x, 10, "-")
  end
end

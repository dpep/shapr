class Fetcher
  def pad(text, width = 10)
    text.ljust(width)
  end

  def go(x)
    pad(x, 10)
  end
end

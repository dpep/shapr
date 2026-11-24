# frozen_string_literal: true

class Fetcher
  def go(x)
    join(x, sep: ",")
  end
end

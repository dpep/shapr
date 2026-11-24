require_relative 'base'

class Child < Base
  def go
    helperThing + 1
  end
end

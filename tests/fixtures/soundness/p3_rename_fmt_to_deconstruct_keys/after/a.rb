class Point
  def initialize(a) = @a = a
  def deconstruct_keys(keys) = { a: @a }
end
